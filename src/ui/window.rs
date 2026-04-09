use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::engine::traits::accounts::MailAccounts;
use epistle::engine::traits::folders::MailFolders;

use super::sidebar::EpistleSidebar;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/window.ui")]
    pub struct EpistleWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub outer_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub inner_split: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub sidebar_toggle: TemplateChild<gtk::ToggleButton>,

        pub(super) sidebar: std::cell::OnceCell<EpistleSidebar>,
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
            let window = self.obj();
            window.setup_sidebar_toggle();
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

    /// Pass engine trait objects to child components, then parent the sidebar.
    ///
    /// The sidebar receives its engine references first, then gets added to the
    /// split view — which triggers root(), where it subscribes to events and
    /// loads cached data.
    pub fn set_engine(&self, accounts: Arc<dyn MailAccounts>, folders: Arc<dyn MailFolders>) {
        let sidebar = EpistleSidebar::new();
        sidebar.set_engine(accounts, folders);

        // Parenting triggers root() — sidebar is now fully wired
        self.imp().outer_split.set_sidebar(Some(&sidebar));
        self.imp().sidebar.set(sidebar).expect("sidebar set once");
    }

    fn setup_sidebar_toggle(&self) {
        let imp = self.imp();
        let split = imp.outer_split.clone();
        imp.sidebar_toggle.connect_toggled(move |btn| {
            split.set_show_sidebar(btn.is_active());
        });

        let toggle = imp.sidebar_toggle.clone();
        imp.outer_split
            .connect_show_sidebar_notify(move |split| {
                toggle.set_active(split.shows_sidebar());
            });
    }
}
