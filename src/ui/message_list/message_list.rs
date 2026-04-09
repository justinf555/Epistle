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
        let store = self.imp().store.get().expect("store initialized");
        let stack = &*self.imp().stack;

        if messages.is_empty() {
            store.remove_all();
            stack.set_visible_child_name("empty");
            return;
        }

        // Replace the store contents with new MessageObjects
        let objects: Vec<MessageObject> = messages.iter().map(MessageObject::new).collect();
        store.remove_all();
        store.extend_from_slice(&objects);

        stack.set_visible_child_name("list");

        tracing::debug!(count = messages.len(), "Message list model updated");
    }
}
