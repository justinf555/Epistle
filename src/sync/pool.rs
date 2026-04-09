use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_imap::types::Mailbox;
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_native_tls::TlsStream;
use tracing::{debug, warn};

use crate::goa::types::ImapConfig;
use crate::goa::GoaClient;
use crate::sync::imap::ImapError;

/// Default max concurrent IMAP connections per account.
const DEFAULT_MAX_CONNECTIONS: usize = 10;

/// Max connections for providers with strict limits (Gmail, Microsoft).
const STRICT_MAX_CONNECTIONS: usize = 5;

/// Connections older than this are discarded (servers typically timeout at 30 min).
const MAX_SESSION_AGE: Duration = Duration::from_secs(25 * 60);

/// Connections idle longer than this are discarded.
const MAX_IDLE_TIME: Duration = Duration::from_secs(5 * 60);

/// How often the reaper checks for stale connections.
const REAP_INTERVAL: Duration = Duration::from_secs(60);

// ── ImapSession ────────────────────────────────────────────────────────────

/// Type-erased IMAP session covering both transport variants.
pub enum ImapSession {
    Tls(async_imap::Session<TlsStream<TcpStream>>),
    Plain(async_imap::Session<TcpStream>),
}

impl ImapSession {
    pub async fn select(&mut self, folder: &str) -> Result<Mailbox, ImapError> {
        match self {
            Self::Tls(s) => Ok(s.select(folder).await?),
            Self::Plain(s) => Ok(s.select(folder).await?),
        }
    }

    pub async fn fetch(
        &mut self,
        range: &str,
        query: &str,
    ) -> Result<Vec<async_imap::types::Fetch>, ImapError> {
        match self {
            Self::Tls(s) => Ok(s.fetch(range, query).await?.try_collect().await?),
            Self::Plain(s) => Ok(s.fetch(range, query).await?.try_collect().await?),
        }
    }

    pub async fn uid_fetch(
        &mut self,
        uid_set: &str,
        query: &str,
    ) -> Result<Vec<async_imap::types::Fetch>, ImapError> {
        match self {
            Self::Tls(s) => Ok(s.uid_fetch(uid_set, query).await?.try_collect().await?),
            Self::Plain(s) => Ok(s.uid_fetch(uid_set, query).await?.try_collect().await?),
        }
    }

    pub async fn list(
        &mut self,
        reference: Option<&str>,
        pattern: Option<&str>,
    ) -> Result<Vec<async_imap::types::Name>, ImapError> {
        match self {
            Self::Tls(s) => Ok(s.list(reference, pattern).await?.try_collect().await?),
            Self::Plain(s) => Ok(s.list(reference, pattern).await?.try_collect().await?),
        }
    }

    pub async fn logout(&mut self) -> Result<(), ImapError> {
        match self {
            Self::Tls(s) => Ok(s.logout().await?),
            Self::Plain(s) => Ok(s.logout().await?),
        }
    }

    pub async fn noop(&mut self) -> Result<(), ImapError> {
        match self {
            Self::Tls(s) => {
                s.noop().await?;
                Ok(())
            }
            Self::Plain(s) => {
                s.noop().await?;
                Ok(())
            }
        }
    }
}

// ── PooledConnection ───────────────────────────────────────────────────────

/// A live IMAP session with lifecycle metadata.
struct PooledConnection {
    session: ImapSession,
    created_at: Instant,
    last_used: Instant,
}

impl PooledConnection {
    fn new(session: ImapSession) -> Self {
        let now = Instant::now();
        Self {
            session,
            created_at: now,
            last_used: now,
        }
    }

    fn is_stale(&self) -> bool {
        self.created_at.elapsed() > MAX_SESSION_AGE
            || self.last_used.elapsed() > MAX_IDLE_TIME
    }
}

// ── AccountPool ────────────────────────────────────────────────────────────

/// Pool of IMAP connections for a single account.
struct AccountPool {
    account_id: String,
    semaphore: Arc<Semaphore>,
    idle_connections: std::sync::Mutex<Vec<PooledConnection>>,
}

impl AccountPool {
    fn new(account_id: String, max_connections: usize) -> Self {
        Self {
            account_id,
            semaphore: Arc::new(Semaphore::new(max_connections)),
            idle_connections: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Take an idle connection from the pool, discarding stale ones.
    fn take_idle(&self) -> Option<PooledConnection> {
        let mut conns = self.idle_connections.lock().unwrap();
        while let Some(conn) = conns.pop() {
            if !conn.is_stale() {
                return Some(conn);
            }
            debug!(
                account_id = %self.account_id,
                age_secs = conn.created_at.elapsed().as_secs(),
                idle_secs = conn.last_used.elapsed().as_secs(),
                "Discarding stale pooled connection"
            );
            // Session dropped without logout — acceptable for stale connections
        }
        None
    }

    /// Return a connection to the pool.
    fn return_connection(&self, mut conn: PooledConnection) {
        conn.last_used = Instant::now();
        self.idle_connections.lock().unwrap().push(conn);
    }

    /// Evict stale connections.
    fn evict_stale(&self) -> usize {
        let mut conns = self.idle_connections.lock().unwrap();
        let before = conns.len();
        conns.retain(|c| !c.is_stale());
        before - conns.len()
    }
}

// ── ConnectionGuard ────────────────────────────────────────────────────────

/// RAII guard returned by [`SyncTaskPool::acquire`].
///
/// Returns the connection to the pool on drop, or discards it if poisoned.
/// The semaphore permit is always released on drop.
pub struct ConnectionGuard {
    connection: Option<PooledConnection>,
    pool: Arc<AccountPool>,
    _permit: OwnedSemaphorePermit,
    poisoned: bool,
}

impl ConnectionGuard {
    /// Access the IMAP session.
    pub fn session(&mut self) -> &mut ImapSession {
        &mut self.connection.as_mut().expect("connection taken").session
    }

    /// Mark this connection as broken. It will be discarded on drop.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(conn) = self.connection.take() {
            if self.poisoned {
                debug!(
                    account_id = %self.pool.account_id,
                    "Discarding poisoned connection"
                );
            } else {
                self.pool.return_connection(conn);
            }
        }
        // _permit is released automatically
    }
}

// ── SyncTaskPool ───────────────────────────────────────────────────────────

/// Top-level IMAP connection pool manager.
///
/// Manages per-account pools with concurrency limits. Connections are reused
/// across operations and health-checked before checkout.
pub struct SyncTaskPool {
    pools: std::sync::Mutex<HashMap<String, Arc<AccountPool>>>,
    goa: Arc<tokio::sync::Mutex<GoaClient>>,
}

impl SyncTaskPool {
    pub fn new(goa: Arc<tokio::sync::Mutex<GoaClient>>) -> Self {
        Self {
            pools: std::sync::Mutex::new(HashMap::new()),
            goa,
        }
    }

    /// Acquire a connection for the given account.
    ///
    /// Blocks if the account's concurrency limit is reached. Reuses idle
    /// connections when available, creating new ones as needed.
    pub async fn acquire(
        &self,
        account_id: &str,
        config: &ImapConfig,
        max_connections: usize,
    ) -> Result<ConnectionGuard, ImapError> {
        let account_pool = self.get_or_create_pool(account_id, max_connections);

        // Block until a permit is available
        let permit = account_pool
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed");

        // Try to reuse an idle connection
        if let Some(mut conn) = account_pool.take_idle() {
            // Health check with NOOP
            match conn.session.noop().await {
                Ok(()) => {
                    debug!(account_id, "Reusing pooled IMAP connection");
                    return Ok(ConnectionGuard {
                        connection: Some(conn),
                        pool: account_pool,
                        _permit: permit,
                        poisoned: false,
                    });
                }
                Err(e) => {
                    debug!(account_id, error = %e, "Pooled connection failed health check, creating new");
                }
            }
        }

        // Create a fresh connection
        let auth = self.goa.lock().await.get_imap_auth(account_id).await
            .map_err(|e| ImapError::Auth(e.to_string()))?;
        let session = crate::sync::imap::connect(config, &auth).await?;

        debug!(account_id, host = %config.host, "Created new pooled IMAP connection");

        Ok(ConnectionGuard {
            connection: Some(PooledConnection::new(session)),
            pool: account_pool,
            _permit: permit,
            poisoned: false,
        })
    }

    /// Spawn a background task that evicts stale connections periodically.
    pub fn spawn_reaper(self: &Arc<Self>) {
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAP_INTERVAL);
            loop {
                interval.tick().await;
                pool.evict_stale();
            }
        });
    }

    /// Shut down all pools, logging out idle connections.
    pub async fn shutdown(&self) {
        let pools: Vec<Arc<AccountPool>> = {
            let map = self.pools.lock().unwrap();
            map.values().cloned().collect()
        };

        for pool in pools {
            let mut conns: Vec<PooledConnection> = {
                pool.idle_connections.lock().unwrap().drain(..).collect()
            };
            for conn in &mut conns {
                if let Err(e) = conn.session.logout().await {
                    warn!(
                        account_id = %pool.account_id,
                        error = %e,
                        "Error during pool shutdown logout"
                    );
                }
            }
        }
    }

    fn get_or_create_pool(&self, account_id: &str, max_connections: usize) -> Arc<AccountPool> {
        let mut pools = self.pools.lock().unwrap();
        pools
            .entry(account_id.to_string())
            .or_insert_with(|| {
                debug!(account_id, max_connections, "Creating account connection pool");
                Arc::new(AccountPool::new(account_id.to_string(), max_connections))
            })
            .clone()
    }

    fn evict_stale(&self) {
        let pools: Vec<Arc<AccountPool>> = {
            self.pools.lock().unwrap().values().cloned().collect()
        };
        for pool in pools {
            let evicted = pool.evict_stale();
            if evicted > 0 {
                debug!(
                    account_id = %pool.account_id,
                    evicted,
                    "Reaped stale connections"
                );
            }
        }
    }
}

/// Return the max connection count for a provider type string.
pub fn max_connections_for_provider(provider_type: &str) -> usize {
    match provider_type {
        "google" | "ms_graph" => STRICT_MAX_CONNECTIONS,
        _ => DEFAULT_MAX_CONNECTIONS,
    }
}
