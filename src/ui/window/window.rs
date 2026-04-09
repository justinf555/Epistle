use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::engine::traits::accounts::MailAccounts;
use epistle::engine::traits::folders::MailFolders;
use epistle::engine::traits::messages::MailMessages;
use epistle::event_bus::EventSender;

use crate::ui::message_list::EpistleMessageList;
use crate::ui::message_list::MessageObject;
use crate::ui::message_view::EpistleMessageView;
use crate::ui::sidebar::EpistleSidebar;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/window/window.ui")]
    pub struct EpistleWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub outer_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub inner_split: TemplateChild<adw::NavigationSplitView>,

        pub(super) sidebar: std::cell::OnceCell<EpistleSidebar>,
        pub(super) message_list: std::cell::OnceCell<EpistleMessageList>,
        pub(super) message_view: std::cell::OnceCell<EpistleMessageView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleWindow {
        const NAME: &'static str = "EpistleWindow";
        type Type = super::EpistleWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EpistleWindow {
        fn constructed(&self) {
            self.parent_constructed();
        }
    }

    impl WidgetImpl for EpistleWindow {}
    impl WindowImpl for EpistleWindow {}
    impl ApplicationWindowImpl for EpistleWindow {}
    impl AdwApplicationWindowImpl for EpistleWindow {}
}

glib::wrapper! {
    pub struct EpistleWindow(ObjectSubclass<imp::EpistleWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl EpistleWindow {
    pub fn new<P: IsA<gtk::Application>>(application: &P) -> Self {
        glib::Object::builder()
            .property("application", application)
            .build()
    }

    pub fn sidebar(&self) -> &EpistleSidebar {
        self.imp().sidebar.get().expect("sidebar initialized")
    }

    /// Pass engine trait objects to child components, then parent them.
    ///
    /// Each component receives its engine references first, then gets added to
    /// the split view — which triggers root(), where it subscribes to events and
    /// loads cached data.
    pub fn set_engine(
        &self,
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
        messages: Arc<dyn MailMessages>,
        sender: EventSender,
    ) {
        // Clone for message list and message view before sidebar takes ownership
        let accounts_for_messages = Arc::clone(&accounts);
        let messages_for_view = Arc::clone(&messages);

        // Sidebar — inject and parent into outer split
        let sidebar = EpistleSidebar::new();
        sidebar.set_engine(accounts, folders);
        self.imp().outer_split.set_sidebar(Some(&sidebar));
        self.imp().sidebar.set(sidebar).expect("sidebar set once");

        // Message list — inject and parent into inner split sidebar
        let message_list = EpistleMessageList::new();
        message_list.set_engine(accounts_for_messages, messages);

        // Wire sidebar toggle to outer split
        self.setup_sidebar_toggle(message_list.sidebar_toggle());

        self.imp().inner_split.set_sidebar(Some(&message_list));

        // Message view — inject and parent into inner split content
        let message_view = EpistleMessageView::new();
        message_view.set_engine(messages_for_view, sender);

        self.imp().inner_split.set_content(Some(&message_view));

        // Wire selection: message list → message view
        self.setup_selection(&message_list, &message_view);

        self.imp()
            .message_list
            .set(message_list)
            .expect("message_list set once");
        self.imp()
            .message_view
            .set(message_view)
            .expect("message_view set once");
    }

    fn setup_sidebar_toggle(&self, toggle: &gtk::ToggleButton) {
        let split = self.imp().outer_split.clone();
        let toggle_clone = toggle.clone();
        toggle.connect_toggled(move |btn| {
            split.set_show_sidebar(btn.is_active());
        });

        let outer_split = self.imp().outer_split.clone();
        outer_split.connect_show_sidebar_notify(move |split| {
            toggle_clone.set_active(split.shows_sidebar());
        });
    }

    fn setup_selection(
        &self,
        message_list: &EpistleMessageList,
        message_view: &EpistleMessageView,
    ) {
        let selection = message_list.selection_model().clone();
        let view = message_view.clone();
        selection.connect_selection_changed(move |sel, _, _| {
            if let Some(item) = sel.selected_item().and_downcast::<MessageObject>() {
                view.show_message(
                    &item.account_id().unwrap_or_default(),
                    &item.folder_name().unwrap_or_default(),
                    item.uid(),
                    item.subject().as_deref(),
                    item.sender().as_deref(),
                    item.date().as_deref(),
                );
            }
        });
    }
}
