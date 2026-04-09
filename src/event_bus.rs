use std::cell::RefCell;
use std::sync::mpsc;

use gtk::glib;

use crate::app_event::AppEvent;

/// Centralised event bus with push-based fan-out subscriber delivery.
///
/// Events are delivered to all subscribers on the GTK main thread via
/// `glib::idle_add_once` — zero CPU when idle, zero polling, no timer.
///
/// The [`sender`](Self::sender) is `Send + Clone` so it can be used from
/// Tokio tasks. Each `send()` schedules a main-loop callback that drains
/// the internal channel and dispatches to all registered subscribers.
///
/// All subscribers must be registered before events begin flowing (i.e. in
/// component constructors during app setup). Calling [`subscribe`](Self::subscribe)
/// from within a subscriber callback will panic due to `RefCell` re-entrancy.
#[derive(Debug)]
pub struct EventBus {
    tx: mpsc::Sender<AppEvent>,
}

type SubscriberList = Vec<Box<dyn Fn(&AppEvent)>>;

thread_local! {
    static SUBSCRIBERS: RefCell<SubscriberList> = const { RefCell::new(Vec::new()) };
    static RECEIVER: RefCell<Option<mpsc::Receiver<AppEvent>>> = const { RefCell::new(None) };
}

/// Drain all pending events from the channel and deliver to subscribers.
fn drain_events() {
    RECEIVER.with(|rx_cell| {
        let rx = rx_cell.borrow();
        let Some(rx) = rx.as_ref() else { return };

        SUBSCRIBERS.with(|subs_cell| {
            let subs = subs_cell.borrow();
            while let Ok(event) = rx.try_recv() {
                for handler in subs.iter() {
                    handler(&event);
                }
            }
        });
    });
}

#[allow(clippy::new_without_default)]
impl EventBus {
    /// Create a new event bus.
    ///
    /// Must be called on the GTK main thread. Only one `EventBus` may exist
    /// per thread (the subscriber list is thread-local).
    pub fn new() -> Self {
        RECEIVER.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "EventBus: only one instance per thread is allowed"
            );
        });

        let (tx, rx) = mpsc::channel::<AppEvent>();

        RECEIVER.with(|cell| {
            *cell.borrow_mut() = Some(rx);
        });

        Self { tx }
    }

    /// Get a sender for producing events.
    ///
    /// The sender is `Send + Clone` — safe to use from Tokio tasks,
    /// background threads, and GTK signal handlers.
    pub fn sender(&self) -> EventSender {
        EventSender {
            tx: self.tx.clone(),
        }
    }

    /// Synchronously drain all pending events on the current thread.
    ///
    /// Used during shutdown when the GLib main loop is exiting and
    /// idle callbacks will no longer fire.
    pub fn drain(&self) {
        drain_events();
    }

    /// Register a subscriber callback. Called on the GTK main thread.
    ///
    /// The subscriber receives every event — use `match` to filter.
    /// Subscribers are called in registration order.
    pub fn subscribe(&self, handler: impl Fn(&AppEvent) + 'static) {
        SUBSCRIBERS.with(|cell| {
            cell.borrow_mut().push(Box::new(handler));
        });
    }
}

/// Subscribe to the event bus from any code running on the GTK main thread.
///
/// Convenience for components that don't have a direct `EventBus` reference.
pub fn subscribe(handler: impl Fn(&AppEvent) + 'static) {
    SUBSCRIBERS.with(|cell| {
        cell.borrow_mut().push(Box::new(handler));
    });
}

impl Drop for EventBus {
    fn drop(&mut self) {
        RECEIVER.with(|cell| {
            cell.borrow_mut().take();
        });
        SUBSCRIBERS.with(|cell| {
            cell.borrow_mut().clear();
        });
    }
}

/// Thread-safe event sender. Cloneable, `Send`.
///
/// Each `send()` pushes the event into an mpsc channel and schedules a
/// `glib::idle_add_once` to drain it on the GTK main thread.
#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<AppEvent>,
}

impl EventSender {
    /// Create a no-op sender for testing. Events are sent but never drained.
    pub fn no_op() -> Self {
        let (tx, _rx) = mpsc::channel();
        Self { tx }
    }

    /// Send an event. Safe to call from any thread.
    ///
    /// The event is delivered to all subscribers on the next GTK main
    /// loop iteration (via `glib::idle_add_once`).
    pub fn send(&self, event: AppEvent) {
        if self.tx.send(event).is_ok() {
            glib::idle_add_once(drain_events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_is_send_and_clone() {
        fn assert_send<T: Send>() {}
        fn assert_clone<T: Clone>() {}
        assert_send::<EventSender>();
        assert_clone::<EventSender>();
    }
}
