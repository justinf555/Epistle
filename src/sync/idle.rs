//! IMAP IDLE manager — maintains persistent connections for real-time
//! push notifications on prioritized folders.
//!
//! Each folder gets one long-lived tokio task that holds a dedicated IMAP
//! connection (not from the pool). On server notification, the task emits
//! an event that triggers differential sync via `sync_folder`.
//!
//! Fallback: when the server doesn't support IDLE, polls on a timer.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::app_event::AppEvent;
use crate::event_bus::EventSender;
use crate::goa::types::{AuthMethod, ImapConfig};
use crate::sync::imap;
use crate::sync::pool::ImapSession;

/// How often to re-enter IDLE (RFC 2177 recommends before 29 min server timeout).
const IDLE_TIMEOUT: Duration = Duration::from_secs(28 * 60);

/// Maximum backoff delay on connection errors.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// IDLE priority order — higher priority folders get IDLE first when budget is constrained.
const IDLE_PRIORITY: &[&str] = &["inbox", "sent", "drafts", "archive", "trash"];

/// Auth provider closure type — returns fresh credentials (handles OAuth token refresh).
pub type AuthProvider =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<AuthMethod>> + Send>> + Send + Sync>;

/// Manages IDLE tasks across all accounts.
pub struct IdleManager {
    tasks: HashMap<(String, String), tokio::task::JoinHandle<()>>,
    shutdown: Arc<Notify>,
}

impl IdleManager {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Start IDLE or poll tasks for an account's folders, respecting connection budget.
    pub fn start_for_account(
        &mut self,
        account_id: &str,
        folders: &[(String, Option<String>)], // (folder_name, role)
        config: ImapConfig,
        auth_provider: AuthProvider,
        idle_budget: usize,
        supports_idle: bool,
        poll_interval: Duration,
        sender: EventSender,
    ) {
        // Sort folders by IDLE priority
        let mut prioritized: Vec<_> = folders.to_vec();
        prioritized.sort_by_key(|(_, role)| {
            role.as_deref()
                .and_then(|r| IDLE_PRIORITY.iter().position(|p| *p == r))
                .unwrap_or(IDLE_PRIORITY.len())
        });

        for (i, (folder_name, role)) in prioritized.iter().enumerate() {
            let key = (account_id.to_string(), folder_name.clone());
            if self.tasks.contains_key(&key) {
                continue;
            }

            let use_idle = supports_idle && i < idle_budget;

            let account_id = account_id.to_string();
            let folder_name = folder_name.clone();
            let config = config.clone();
            let auth_provider = Arc::clone(&auth_provider);
            let sender = sender.clone();
            let shutdown = Arc::clone(&self.shutdown);

            if use_idle {
                info!(
                    account_id = %account_id,
                    folder = %folder_name,
                    role = ?role,
                    "Starting IDLE task"
                );

                let handle = tokio::spawn(async move {
                    idle_loop(&account_id, &folder_name, &config, auth_provider, sender, shutdown)
                        .await;
                });

                self.tasks.insert(key, handle);
            } else {
                info!(
                    account_id = %account_id,
                    folder = %folder_name,
                    role = ?role,
                    interval_secs = poll_interval.as_secs(),
                    reason = if !supports_idle { "no IDLE capability" } else { "over budget" },
                    "Starting poll task"
                );

                let handle = tokio::spawn(async move {
                    poll_loop(&account_id, &folder_name, poll_interval, sender, shutdown).await;
                });

                self.tasks.insert(key, handle);
            }
        }
    }

    /// Signal all IDLE/poll tasks to shut down.
    pub fn shutdown(&self) {
        info!("Shutting down IDLE manager");
        self.shutdown.notify_waiters();
    }
}

/// Core IDLE loop for a single folder. Reconnects with exponential backoff.
async fn idle_loop(
    account_id: &str,
    folder_name: &str,
    config: &ImapConfig,
    auth_provider: AuthProvider,
    sender: EventSender,
    shutdown: Arc<Notify>,
) {
    let mut backoff = Duration::from_secs(1);

    loop {
        // Get fresh auth (tokens may have expired)
        let auth = match (auth_provider)().await {
            Ok(a) => a,
            Err(e) => {
                error!(account_id, folder_name, error = %e, "Failed to get auth for IDLE");
                if !sleep_or_shutdown(backoff, &shutdown).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Connect
        let session = match imap::connect(config, &auth).await {
            Ok(s) => s,
            Err(e) => {
                warn!(account_id, folder_name, error = %e, "IDLE connection failed, retrying");
                if !sleep_or_shutdown(backoff, &shutdown).await {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // Reset backoff on successful connect
        backoff = Duration::from_secs(1);
        info!(account_id, folder_name, "IDLE connection established");

        // Run the IDLE session — dispatch to concrete transport type
        match session {
            ImapSession::Tls(s) => {
                run_idle_session(s, account_id, folder_name, &sender, &shutdown).await;
            }
            ImapSession::Plain(s) => {
                run_idle_session(s, account_id, folder_name, &sender, &shutdown).await;
            }
        }

        debug!(account_id, folder_name, "IDLE session ended, will reconnect");
    }
}

/// Run IDLE on a concrete session type. Loops: SELECT → IDLE → wait → emit → DONE → repeat.
async fn run_idle_session<T>(
    mut session: async_imap::Session<T>,
    account_id: &str,
    folder_name: &str,
    sender: &EventSender,
    shutdown: &Arc<Notify>,
) where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    if let Err(e) = session.select(folder_name).await {
        error!(account_id, folder_name, error = %e, "Failed to SELECT for IDLE");
        return;
    }

    loop {
        // Enter IDLE — consumes the session
        let mut handle = session.idle();
        if let Err(e) = handle.init().await {
            error!(account_id, folder_name, error = %e, "Failed to init IDLE");
            return;
        }

        debug!(account_id, folder_name, "Entered IDLE");

        // Wait for notification or timeout
        let (idle_fut, stop_source) = handle.wait_with_timeout(IDLE_TIMEOUT);

        // Spawn a task that drops the StopSource on shutdown, interrupting IDLE
        let shutdown_clone = Arc::clone(shutdown);
        let interrupt_task = tokio::spawn(async move {
            shutdown_clone.notified().await;
            drop(stop_source);
        });

        let response = idle_fut.await;
        interrupt_task.abort();

        match response {
            Ok(async_imap::extensions::idle::IdleResponse::NewData(data)) => {
                debug!(
                    account_id,
                    folder_name,
                    response = ?data.parsed(),
                    "IDLE notification received"
                );

                session = match handle.done().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!(account_id, folder_name, error = %e, "Failed to exit IDLE");
                        return;
                    }
                };

                sender.send(AppEvent::IdleNotification {
                    account_id: account_id.to_string(),
                    folder_name: folder_name.to_string(),
                });
            }
            Ok(async_imap::extensions::idle::IdleResponse::Timeout) => {
                debug!(account_id, folder_name, "IDLE timeout, re-entering");
                session = match handle.done().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!(account_id, folder_name, error = %e, "Failed to exit IDLE after timeout");
                        return;
                    }
                };
            }
            Ok(async_imap::extensions::idle::IdleResponse::ManualInterrupt) => {
                debug!(account_id, folder_name, "IDLE interrupted (shutdown)");
                let _ = handle.done().await;
                return;
            }
            Err(e) => {
                error!(account_id, folder_name, error = %e, "IDLE error");
                return;
            }
        }
    }
}

/// Poll loop fallback for servers without IDLE capability.
async fn poll_loop(
    account_id: &str,
    folder_name: &str,
    interval: Duration,
    sender: EventSender,
    shutdown: Arc<Notify>,
) {
    loop {
        if !sleep_or_shutdown(interval, &shutdown).await {
            debug!(account_id, folder_name, "Poll task shutting down");
            return;
        }

        debug!(account_id, folder_name, "Poll timer fired");
        sender.send(AppEvent::IdleNotification {
            account_id: account_id.to_string(),
            folder_name: folder_name.to_string(),
        });
    }
}

/// Sleep for `duration`, returning `true` if completed, `false` if shutdown.
async fn sleep_or_shutdown(duration: Duration, shutdown: &Arc<Notify>) -> bool {
    tokio::select! {
        biased;
        _ = shutdown.notified() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}
