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

            // Create the model (ListStore) and wire it with factory to ListView
            let store = gio::ListStore::new::<MessageObject>();
            let selection = gtk::SingleSelection::new(Some(store.clone()));
            self.list_view.set_model(Some(&selection));
            self.list_view.set_factory(Some(&factory::build_factory()));
            self.store.set(store).expect("store set once in constructed");
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

    fn subscribe_events(&self) {
        let weak = self.downgrade();
        epistle::event_bus::subscribe(move |event| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::MessagesChanged {
                    folder_name,
                    messages,
                    ..
                } => {
                    if folder_name == "INBOX" {
                        list.on_messages_changed(messages);
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
                        list.on_messages_changed(&cached_messages);
                        return;
                    }
                }
            }
        });
    }

    fn on_messages_changed(&self, messages: &[Message]) {
        let imp = self.imp();
        let store = imp.store.get().expect("store initialized");
        let stack = &*imp.stack;
        let mut index = imp.uid_index.borrow_mut();

        if messages.is_empty() {
            store.remove_all();
            index.clear();
            stack.set_visible_child_name("empty");
            return;
        }

        // Build set of incoming UIDs for O(1) membership test
        let incoming: HashMap<u32, &Message> = messages.iter().map(|m| (m.uid, m)).collect();

        // Pass 1: Update existing items in-place, collect stale indices
        let mut stale_positions: Vec<u32> = Vec::new();
        for (&uid, &pos) in index.iter() {
            if let Some(msg) = incoming.get(&uid) {
                let obj = store.item(pos).and_downcast::<MessageObject>().unwrap();
                obj.update_from(msg);
            } else {
                stale_positions.push(pos);
            }
        }

        // Pass 2: Remove stale items via splice (highest index first to keep positions valid)
        stale_positions.sort_unstable_by(|a, b| b.cmp(a));
        for pos in &stale_positions {
            let obj = store.item(*pos).and_downcast::<MessageObject>().unwrap();
            index.remove(&obj.uid());
            store.remove(*pos);
        }

        // Rebuild index positions after removals (positions shifted)
        index.clear();
        for i in 0..store.n_items() {
            let obj = store.item(i).and_downcast::<MessageObject>().unwrap();
            index.insert(obj.uid(), i);
        }

        // Pass 3: Collect new items not in the store
        let new_items: Vec<MessageObject> = messages
            .iter()
            .filter(|m| !index.contains_key(&m.uid))
            .map(MessageObject::new)
            .collect();

        if !new_items.is_empty() {
            let insert_pos = store.n_items();
            store.splice(insert_pos, 0, &new_items);

            // Update index with new positions
            for (i, item) in new_items.iter().enumerate() {
                index.insert(item.uid(), insert_pos + i as u32);
            }
        }

        stack.set_visible_child_name("list");

        tracing::debug!(
            count = store.n_items(),
            updated = messages.len().saturating_sub(new_items.len()),
            added = new_items.len(),
            removed = stale_positions.len(),
            "Message list model updated"
        );
    }
}
