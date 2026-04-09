use std::cell::{Cell, RefCell};
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use webkit6::prelude::*;

use epistle::app_event::AppEvent;
use epistle::engine::pipeline::sanitise;
use epistle::engine::traits::messages::{MailMessages, MessageBody};
use epistle::event_bus::EventSender;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/message_view/message_view.ui")]
    pub struct EpistleMessageView {
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) content_box: TemplateChild<gtk::Box>,

        pub(super) messages: std::cell::OnceCell<Arc<dyn MailMessages>>,
        pub(super) sender: std::cell::OnceCell<EventSender>,
        pub(super) webview: RefCell<Option<webkit6::WebView>>,
        pub(super) current_uid: Cell<u32>,
        pub(super) current_account_id: RefCell<String>,
        pub(super) current_folder: RefCell<String>,

        // Header widgets (added dynamically to content_box)
        pub(super) header_box: std::cell::OnceCell<gtk::Box>,
        pub(super) subject_label: std::cell::OnceCell<gtk::Label>,
        pub(super) from_label: std::cell::OnceCell<gtk::Label>,
        pub(super) date_label: std::cell::OnceCell<gtk::Label>,
    }

    impl std::fmt::Debug for EpistleMessageView {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EpistleMessageView").finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleMessageView {
        const NAME: &'static str = "EpistleMessageView";
        type Type = super::EpistleMessageView;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EpistleMessageView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.build_header();
        }
    }

    impl WidgetImpl for EpistleMessageView {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            obj.subscribe_events();
        }

        fn unroot(&self) {
            self.parent_unroot();
        }
    }

    impl NavigationPageImpl for EpistleMessageView {}
}

glib::wrapper! {
    pub struct EpistleMessageView(ObjectSubclass<imp::EpistleMessageView>)
        @extends gtk::Widget, adw::NavigationPage,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EpistleMessageView {
    fn default() -> Self {
        Self::new()
    }
}

impl EpistleMessageView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Inject engine trait objects. Must be called before the widget is rooted.
    pub fn set_engine(&self, messages: Arc<dyn MailMessages>, sender: EventSender) {
        self.imp()
            .messages
            .set(messages)
            .ok()
            .expect("messages set once");
        self.imp()
            .sender
            .set(sender)
            .ok()
            .expect("sender set once");
    }

    /// Show a message. Displays header immediately, then fetches body.
    pub fn show_message(
        &self,
        account_id: &str,
        folder_name: &str,
        uid: u32,
        subject: Option<&str>,
        sender_display: Option<&str>,
        date: Option<&str>,
    ) {
        let imp = self.imp();

        // Track current message for race condition protection
        imp.current_uid.set(uid);
        *imp.current_account_id.borrow_mut() = account_id.to_string();
        *imp.current_folder.borrow_mut() = folder_name.to_string();

        // Show header immediately
        if let Some(label) = imp.subject_label.get() {
            label.set_text(subject.unwrap_or("(no subject)"));
        }
        if let Some(label) = imp.from_label.get() {
            label.set_text(sender_display.unwrap_or("(unknown)"));
        }
        if let Some(label) = imp.date_label.get() {
            label.set_text(date.unwrap_or(""));
        }

        // Clear previous webview content
        if let Some(wv) = imp.webview.borrow().as_ref() {
            wv.load_html("", None);
        }

        // Show loading state
        imp.stack.set_visible_child_name("loading");

        // Try to load from cache first, then fall back to IMAP fetch
        self.load_body(account_id, folder_name, uid);
    }

    fn load_body(&self, account_id: &str, folder_name: &str, uid: u32) {
        let messages = Arc::clone(
            self.imp()
                .messages
                .get()
                .expect("engine set before use"),
        );
        let sender = self
            .imp()
            .sender
            .get()
            .expect("sender set before use")
            .clone();
        let account_id = account_id.to_string();
        let folder_name = folder_name.to_string();

        let weak = self.downgrade();
        glib::MainContext::default().spawn_local(async move {
            let Some(view) = weak.upgrade() else {
                return;
            };

            // Check if user has already navigated away
            if view.imp().current_uid.get() != uid {
                return;
            }

            // Check DB cache
            if let Ok(Some(body)) = messages.get_body(&account_id, &folder_name, uid).await {
                if body.body_text.is_some() || body.body_html.is_some() {
                    tracing::debug!(uid, "Body loaded from cache");
                    view.render_body(&body);
                    return;
                }
            }

            // Cache miss — request body fetch from SyncEngine
            tracing::debug!(uid, "Body not cached, requesting fetch");
            sender.send(AppEvent::MessageBodyRequested {
                account_id,
                folder_name,
                uid,
            });
        });
    }

    fn subscribe_events(&self) {
        let weak = self.downgrade();
        epistle::event_bus::subscribe(move |event| {
            let Some(view) = weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::FolderSelected { .. } => {
                    view.reset();
                }
                AppEvent::MessageSelected {
                    account_id,
                    folder_name,
                    uid,
                    subject,
                    sender,
                    date,
                } => {
                    view.show_message(
                        account_id,
                        folder_name,
                        *uid,
                        subject.as_deref(),
                        sender.as_deref(),
                        date.as_deref(),
                    );
                }
                AppEvent::MessageBodyFetched {
                    account_id,
                    folder_name,
                    uid,
                    body,
                } => {
                    let imp = view.imp();
                    // Only render if this is still the currently viewed message
                    if imp.current_uid.get() == *uid
                        && *imp.current_account_id.borrow() == *account_id
                        && *imp.current_folder.borrow() == *folder_name
                    {
                        tracing::debug!(uid, "Body fetched, rendering");
                        view.render_body(body);
                    }
                }
                _ => {}
            }
        });
    }

    /// Reset to empty state (e.g., when folder changes).
    fn reset(&self) {
        let imp = self.imp();
        imp.current_uid.set(0);
        *imp.current_account_id.borrow_mut() = String::new();
        *imp.current_folder.borrow_mut() = String::new();
        if let Some(label) = imp.subject_label.get() {
            label.set_text("");
        }
        if let Some(label) = imp.from_label.get() {
            label.set_text("");
        }
        if let Some(label) = imp.date_label.get() {
            label.set_text("");
        }
        if let Some(wv) = imp.webview.borrow().as_ref() {
            wv.load_html("", None);
        }
        imp.stack.set_visible_child_name("empty");
    }

    fn render_body(&self, body: &MessageBody) {
        let imp = self.imp();

        let html = if let Some(ref html_body) = body.body_html {
            sanitise::sanitise_html(html_body)
        } else if let Some(ref text_body) = body.body_text {
            sanitise::plain_text_to_html(text_body)
        } else {
            sanitise::plain_text_to_html("(no content)")
        };

        let webview = self.ensure_webview();
        webview.load_html(&html, None);

        imp.stack.set_visible_child_name("content");
    }

    /// Create or return the WebKitWebView with security settings.
    fn ensure_webview(&self) -> webkit6::WebView {
        let imp = self.imp();
        if let Some(wv) = imp.webview.borrow().as_ref() {
            return wv.clone();
        }

        let settings = webkit6::Settings::new();
        settings.set_enable_javascript(false);
        settings.set_allow_modal_dialogs(false);
        settings.set_enable_developer_extras(false);

        let webview = webkit6::WebView::builder()
            .settings(&settings)
            .vexpand(true)
            .hexpand(true)
            .build();

        // Open links externally via Flatpak portal
        webview.connect_decide_policy(
            |_wv, decision, decision_type| {
                if decision_type == webkit6::PolicyDecisionType::NavigationAction {
                    if let Some(nav) =
                        decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
                    {
                        if let Some(action) = nav.navigation_action() {
                            if let Some(req) = action.request() {
                                if let Some(uri) = req.uri() {
                                    if !uri.is_empty()
                                        && uri.as_str() != "about:blank"
                                        && !uri.starts_with("data:")
                                    {
                                        let launcher =
                                            gtk::UriLauncher::new(&uri);
                                        launcher.launch(
                                            None::<&gtk::Window>,
                                            None::<&gtk::gio::Cancellable>,
                                            |_| {},
                                        );
                                        decision.ignore();
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
                false
            },
        );

        imp.content_box.append(&webview);
        *imp.webview.borrow_mut() = Some(webview.clone());
        webview
    }

    /// Build the message header area (subject, from, date).
    fn build_header(&self) {
        let imp = self.imp();

        let header_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        header_box.set_margin_start(16);
        header_box.set_margin_end(16);
        header_box.set_margin_top(12);
        header_box.set_margin_bottom(12);

        let subject_label = gtk::Label::new(None);
        subject_label.set_xalign(0.0);
        subject_label.set_wrap(true);
        subject_label.add_css_class("title-2");
        header_box.append(&subject_label);

        let from_label = gtk::Label::new(None);
        from_label.set_xalign(0.0);
        from_label.add_css_class("dim-label");
        header_box.append(&from_label);

        let date_label = gtk::Label::new(None);
        date_label.set_xalign(0.0);
        date_label.add_css_class("dim-label");
        date_label.add_css_class("caption");
        header_box.append(&date_label);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(8);
        header_box.append(&separator);

        imp.content_box.append(&header_box);
        imp.header_box.set(header_box).ok();
        imp.subject_label.set(subject_label).ok();
        imp.from_label.set(from_label).ok();
        imp.date_label.set(date_label).ok();
    }
}
