use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use epistle::app_event::AppEvent;
use epistle::event_bus::EventBus;
use epistle::engine::traits::accounts::Account;
use epistle::engine::traits::folders::Folder;

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

    /// Subscribe to the event bus for account and folder updates.
    pub fn subscribe_events(&self, bus: &EventBus) {
        let sidebar_weak = self.downgrade();

        bus.subscribe(move |event| {
            let Some(sidebar) = sidebar_weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::AccountsChanged { accounts } => {
                    sidebar.on_accounts_changed(accounts);
                }
                AppEvent::FoldersChanged {
                    email_address,
                    folders,
                    ..
                } => {
                    sidebar.on_folders_changed(email_address, folders);
                }
                _ => {}
            }
        });
    }

    fn on_accounts_changed(&self, accounts: &[Account]) {
        let sidebar = self.imp().sidebar.get().expect("sidebar initialized");

        // Add empty sections for each account (folders arrive later)
        for account in accounts {
            let section = adw::SidebarSection::new();
            section.set_title(Some(&account.email_address));
            sidebar.append(section);
        }
    }

    fn on_folders_changed(&self, email_address: &str, folders: &[Folder]) {
        let sidebar = self.imp().sidebar.get().expect("sidebar initialized");

        // Find the section for this account by title
        let sections = sidebar.sections();
        for i in 0..sections.n_items() {
            let Some(section) = sidebar.section(i) else {
                continue;
            };
            if section.title().as_deref() == Some(email_address) {
                section.remove_all();
                for folder in folders {
                    let icon = icon_for_role(folder.role.as_deref());
                    let display_name =
                        display_name_for_folder(&folder.name, folder.role.as_deref());
                    let item = adw::SidebarItem::builder()
                        .title(&display_name)
                        .icon_name(icon)
                        .build();
                    section.append(item);
                }
                return;
            }
        }
    }
}

fn icon_for_role(role: Option<&str>) -> &'static str {
    match role {
        Some("inbox") => "mail-inbox-symbolic",
        Some("sent") => "mail-send-symbolic",
        Some("drafts") => "accessories-text-editor-symbolic",
        Some("archive") => "folder-symbolic",
        Some("trash") => "user-trash-symbolic",
        Some("junk") => "dialog-warning-symbolic",
        _ => "folder-symbolic",
    }
}

/// Show a friendly name for standard roles, or the last path component for custom folders.
fn display_name_for_folder(name: &str, role: Option<&str>) -> String {
    match role {
        Some("inbox") => "Inbox".to_string(),
        Some("sent") => "Sent".to_string(),
        Some("drafts") => "Drafts".to_string(),
        Some("archive") => "Archive".to_string(),
        Some("trash") => "Trash".to_string(),
        Some("junk") => "Junk".to_string(),
        _ => {
            name.rsplit_once('/')
                .or_else(|| name.rsplit_once('.'))
                .map(|(_, last)| last.to_string())
                .unwrap_or_else(|| name.to_string())
        }
    }
}
