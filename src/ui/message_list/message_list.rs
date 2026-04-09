use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use epistle::app_event::AppEvent;
use epistle::engine::traits::accounts::MailAccounts;
use epistle::engine::traits::messages::{MailMessages, Message};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/message_list/message_list.ui")]
    pub struct EpistleMessageList {
        #[template_child]
        pub(super) list_box: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) sidebar_toggle: TemplateChild<gtk::ToggleButton>,

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
        let list_box = &*self.imp().list_box;
        let stack = &*self.imp().stack;

        // Clear existing rows
        while let Some(child) = list_box.first_child() {
            list_box.remove(&child);
        }

        if messages.is_empty() {
            stack.set_visible_child_name("empty");
            return;
        }

        stack.set_visible_child_name("list");

        for msg in messages {
            let row = build_message_row(msg);
            list_box.append(&row);
        }

        tracing::debug!(count = messages.len(), "Message list updated");
    }
}

// ── Row building (dynamic — not in template) ────────────────────────────────

fn build_message_row(msg: &Message) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(true);

    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    outer.set_margin_top(8);
    outer.set_margin_bottom(8);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    // Avatar
    let name = display_sender(msg);
    let avatar = adw::Avatar::new(32, Some(&name), true);
    outer.append(&avatar);

    // Text content
    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_box.set_hexpand(true);

    // Top row: sender + timestamp
    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);

    let sender_label = gtk::Label::new(Some(&name));
    sender_label.set_xalign(0.0);
    sender_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sender_label.set_hexpand(true);
    if !msg.is_read {
        sender_label.add_css_class("heading");
    }
    top_row.append(&sender_label);

    let time_label = gtk::Label::new(Some(&format_timestamp(msg.date.as_deref())));
    time_label.set_xalign(1.0);
    time_label.add_css_class("dim-label");
    time_label.add_css_class("caption");
    top_row.append(&time_label);

    text_box.append(&top_row);

    // Subject
    let subject_label = gtk::Label::new(Some(
        msg.subject.as_deref().unwrap_or("(no subject)"),
    ));
    subject_label.set_xalign(0.0);
    subject_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if !msg.is_read {
        subject_label.add_css_class("heading");
    }
    text_box.append(&subject_label);

    // Preview
    if let Some(ref preview) = msg.preview {
        let preview_label = gtk::Label::new(Some(preview));
        preview_label.set_xalign(0.0);
        preview_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        preview_label.add_css_class("dim-label");
        preview_label.add_css_class("caption");
        text_box.append(&preview_label);
    }

    outer.append(&text_box);

    // Flagged star
    if msg.is_flagged {
        let star = gtk::Image::from_icon_name("starred-symbolic");
        star.add_css_class("warning");
        outer.append(&star);
    }

    row.set_child(Some(&outer));
    row
}

fn display_sender(msg: &Message) -> String {
    match &msg.sender {
        Some(sender) => {
            if let Some(idx) = sender.find(" <") {
                sender[..idx].to_string()
            } else {
                sender.clone()
            }
        }
        None => "(unknown)".to_string(),
    }
}

fn format_timestamp(date: Option<&str>) -> String {
    match date {
        Some(d) => {
            if d.len() > 16 {
                d[..16].to_string()
            } else {
                d.to_string()
            }
        }
        None => String::new(),
    }
}
