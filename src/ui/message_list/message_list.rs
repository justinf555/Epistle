use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::app_event::AppEvent;
use epistle::engine::traits::accounts::MailAccounts;
use epistle::engine::traits::messages::{MailMessages, Message};

use super::factory;
use super::item::MessageObject;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/message_list/message_list.ui")]
    pub struct EpistleMessageList {
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) sidebar_toggle: TemplateChild<gtk::ToggleButton>,

        pub(super) store: std::cell::OnceCell<gio::ListStore>,
        pub(super) selection: std::cell::OnceCell<gtk::SingleSelection>,
        /// UID → store index for O(1) lookups.
        pub(super) uid_index: RefCell<HashMap<u32, u32>>,
        pub(super) accounts: std::cell::OnceCell<Arc<dyn MailAccounts>>,
        pub(super) messages: std::cell::OnceCell<Arc<dyn MailMessages>>,
    }

    impl std::fmt::Debug for EpistleMessageList {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EpistleMessageList").finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleMessageList {
        const NAME: &'static str = "EpistleMessageList";
        type Type = super::EpistleMessageList;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EpistleMessageList {
        fn constructed(&self) {
            self.parent_constructed();

            // Create the model chain: ListStore → SingleSelection → ListView
            // ListStore is kept in DB sort order (newest first). New messages are
            // inserted at the correct position via binary search, not appended.
            let store = gio::ListStore::new::<MessageObject>();
            let selection = gtk::SingleSelection::new(Some(store.clone()));
            self.list_view.set_model(Some(&selection));
            self.list_view.set_factory(Some(&factory::build_factory()));
            self.store.set(store).expect("store set once in constructed");
            self.selection
                .set(selection)
                .expect("selection set once in constructed");
        }
    }

    impl WidgetImpl for EpistleMessageList {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            obj.subscribe_events();
            obj.load_cached();
        }

        fn unroot(&self) {
            self.parent_unroot();
        }
    }

    impl NavigationPageImpl for EpistleMessageList {}
}

glib::wrapper! {
    pub struct EpistleMessageList(ObjectSubclass<imp::EpistleMessageList>)
        @extends gtk::Widget, adw::NavigationPage,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EpistleMessageList {
    fn default() -> Self {
        Self::new()
    }
}

impl EpistleMessageList {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Inject engine trait objects. Must be called before the widget is rooted.
    pub fn set_engine(&self, accounts: Arc<dyn MailAccounts>, messages: Arc<dyn MailMessages>) {
        self.imp()
            .accounts
            .set(accounts)
            .ok()
            .expect("accounts set once");
        self.imp()
            .messages
            .set(messages)
            .ok()
            .expect("messages set once");
    }

    /// Access the sidebar toggle button for wiring to the outer split view.
    pub fn sidebar_toggle(&self) -> &gtk::ToggleButton {
        &self.imp().sidebar_toggle
    }

    /// Access the selection model for wiring selection changes.
    pub fn selection_model(&self) -> &gtk::SingleSelection {
        self.imp()
            .selection
            .get()
            .expect("selection initialized")
    }

    fn subscribe_events(&self) {
        let weak = self.downgrade();
        epistle::event_bus::subscribe(move |event| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::MessagesAdded {
                    folder_name,
                    messages,
                    ..
                } => {
                    if folder_name == "INBOX" {
                        list.on_messages_added(messages);
                    }
                }
                AppEvent::MessagesUpdated {
                    folder_name,
                    messages,
                    ..
                } => {
                    if folder_name == "INBOX" {
                        list.on_messages_updated(messages);
                    }
                }
                AppEvent::MessagesRemoved {
                    folder_name,
                    uids,
                    ..
                } => {
                    if folder_name == "INBOX" {
                        list.on_messages_removed(uids);
                    }
                }
                _ => {}
            }
        });
    }

    /// Load cached INBOX messages on startup. Called from root().
    fn load_cached(&self) {
        let accounts = Arc::clone(
            self.imp()
                .accounts
                .get()
                .expect("engine set before root"),
        );
        let messages = Arc::clone(
            self.imp()
                .messages
                .get()
                .expect("engine set before root"),
        );

        let weak = self.downgrade();
        glib::MainContext::default().spawn_local(async move {
            let Some(list) = weak.upgrade() else {
                return;
            };

            let Ok(cached_accounts) = accounts.list_accounts().await else {
                return;
            };

            for account in &cached_accounts {
                if let Ok(cached_messages) = messages.list_messages(&account.goa_id, "INBOX").await
                {
                    if !cached_messages.is_empty() {
                        tracing::debug!(
                            account = %account.email_address,
                            count = cached_messages.len(),
                            "Loaded cached INBOX messages"
                        );
                        list.on_messages_added(&cached_messages);
                        return;
                    }
                }
            }
        });
    }

    /// O(k log n) — insert new messages at the correct sorted position.
    /// Messages from DB arrive in sorted order (newest first). New messages
    /// are typically newest, so insertion at position 0 is the common case.
    fn on_messages_added(&self, messages: &[Message]) {
        let imp = self.imp();
        let store = imp.store.get().expect("store initialized");
        let mut index = imp.uid_index.borrow_mut();

        for msg in messages {
            let item = MessageObject::new(msg);
            let uid = item.uid();

            // Binary search for insertion point (store is sorted newest-first by timestamp)
            let pos = find_insert_position(store, item.sort_timestamp());

            // Shift all existing index entries at or after the insertion point
            for val in index.values_mut() {
                if *val >= pos {
                    *val += 1;
                }
            }

            store.insert(pos, &item);
            index.insert(uid, pos);
        }

        if store.n_items() > 0 {
            imp.stack.set_visible_child_name("list");
        }

        tracing::debug!(added = messages.len(), total = store.n_items(), "Messages added");
    }

    /// O(k) — update existing messages in place via index lookup.
    fn on_messages_updated(&self, messages: &[Message]) {
        let imp = self.imp();
        let store = imp.store.get().expect("store initialized");
        let index = imp.uid_index.borrow();

        for msg in messages {
            if let Some(&pos) = index.get(&msg.uid) {
                if let Some(obj) = store.item(pos).and_downcast::<MessageObject>() {
                    obj.update_from(msg);
                }
            }
        }

        tracing::debug!(updated = messages.len(), "Messages updated");
    }

    /// Remove messages by UID.
    ///
    /// UID lookups are O(1) via the index. Store removals are O(k) where k = removed count.
    /// Index rebuild is O(n) over the full store — chosen over per-removal shift (O(k·n))
    /// because bulk expunge during sync makes the rebuild cheaper for k > 1.
    fn on_messages_removed(&self, uids: &[u32]) {
        let imp = self.imp();
        let store = imp.store.get().expect("store initialized");
        let mut index = imp.uid_index.borrow_mut();

        // Collect positions to remove (sorted descending to preserve indices during removal)
        let mut positions: Vec<u32> = uids
            .iter()
            .filter_map(|uid| index.remove(uid))
            .collect();
        positions.sort_unstable_by(|a, b| b.cmp(a));

        for pos in &positions {
            store.remove(*pos);
        }

        // Rebuild index: O(n) is cheaper than per-removal shift O(k·n) for bulk expunge
        if !positions.is_empty() {
            index.clear();
            for i in 0..store.n_items() {
                if let Some(obj) = store.item(i).and_downcast::<MessageObject>() {
                    index.insert(obj.uid(), i);
                }
            }
        }

        if store.n_items() == 0 {
            imp.stack.set_visible_child_name("empty");
        }

        tracing::debug!(removed = uids.len(), total = store.n_items(), "Messages removed");
    }
}

/// Binary search for the insertion point in a newest-first sorted store.
/// Uses pre-computed Unix timestamps for correct ordering regardless of date format.
fn find_insert_position(store: &gio::ListStore, timestamp: i64) -> u32 {
    let n = store.n_items();
    if n == 0 {
        return 0;
    }

    let mut low: u32 = 0;
    let mut high: u32 = n;

    while low < high {
        let mid = low + (high - low) / 2;
        let mid_obj = store
            .item(mid)
            .and_downcast::<MessageObject>()
            .expect("ListStore contains only MessageObject");
        let mid_ts = mid_obj.sort_timestamp();

        // Store is sorted descending (newest first).
        // If timestamp >= mid, insert before mid (go left).
        // If timestamp < mid, insert after mid (go right).
        if timestamp >= mid_ts {
            high = mid;
        } else {
            low = mid + 1;
        }
    }

    low
}
