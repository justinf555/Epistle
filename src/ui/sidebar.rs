use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use epistle::engine::db::accounts::AccountRow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct EpistleSidebar {
        pub(super) sidebar: std::cell::OnceCell<adw::Sidebar>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleSidebar {
        const NAME: &'static str = "EpistleSidebar";
        type Type = super::EpistleSidebar;
        type ParentType = adw::NavigationPage;
    }

    impl ObjectImpl for EpistleSidebar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_title("Folders");

            let toolbar = adw::ToolbarView::new();

            let header = adw::HeaderBar::new();
            header.set_show_title(false);

            let compose_btn = gtk::Button::new();
            compose_btn.set_icon_name("mail-message-new-symbolic");
            compose_btn.set_tooltip_text(Some("Compose"));
            header.pack_start(&compose_btn);

            let menu_btn = gtk::MenuButton::new();
            menu_btn.set_primary(true);
            menu_btn.set_icon_name("open-menu-symbolic");
            menu_btn.set_tooltip_text(Some("Main Menu"));
            if let Some(menu) = gtk::Builder::from_resource(
                "/io/github/justinf555/Epistle/shortcuts-dialog.ui",
            )
            .object::<gtk::gio::MenuModel>("primary_menu")
            {
                menu_btn.set_menu_model(Some(&menu));
            }
            header.pack_end(&menu_btn);

            toolbar.add_top_bar(&header);

            let sidebar = adw::Sidebar::new();
            sidebar.append(Self::build_unified_section());
            toolbar.set_content(Some(&sidebar));

            obj.set_child(Some(&toolbar));
            self.sidebar.set(sidebar).expect("set once in constructed");
        }
    }

    impl WidgetImpl for EpistleSidebar {}
    impl NavigationPageImpl for EpistleSidebar {}

    impl EpistleSidebar {
        fn build_unified_section() -> adw::SidebarSection {
            let section = adw::SidebarSection::new();
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
                section.append(item);
            }
            section
        }
    }
}

glib::wrapper! {
    pub struct EpistleSidebar(ObjectSubclass<imp::EpistleSidebar>)
        @extends gtk::Widget, adw::NavigationPage,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EpistleSidebar {
    fn default() -> Self {
        Self::new()
    }
}

impl EpistleSidebar {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Add per-account sections to the sidebar from database rows.
    pub fn populate_accounts(&self, accounts: &[AccountRow]) {
        let sidebar = self.imp().sidebar.get().expect("sidebar initialized");

        for account in accounts {
            let section = adw::SidebarSection::new();
            section.set_title(Some(&account.email_address));

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
                section.append(item);
            }

            sidebar.append(section);
        }
    }
}
