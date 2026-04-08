use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

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
        pub sidebar_toolbar: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub sidebar_toggle: TemplateChild<gtk::ToggleButton>,
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
            window.setup_sidebar();
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

    fn setup_sidebar(&self) {
        let sidebar = adw::Sidebar::new();

        let folders = adw::SidebarSection::new();
        for (label, icon) in &[
            ("Inbox", "mail-inbox-symbolic"),
            ("Sent", "mail-send-symbolic"),
            ("Drafts", "accessories-text-editor-symbolic"),
            ("Archive", "folder-symbolic"),
            ("Trash", "user-trash-symbolic"),
        ] {
            let item = adw::SidebarItem::builder()
                .title(*label)
                .icon_name(*icon)
                .build();
            folders.append(item);
        }
        sidebar.append(folders);

        self.imp().sidebar_toolbar.set_content(Some(&sidebar));
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
