use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use epistle::app_event::AppEvent;
use epistle::engine::traits::accounts::{Account, MailAccounts};
use epistle::engine::traits::folders::{Folder, MailFolders};
use epistle::event_bus::EventSender;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/justinf555/Epistle/ui/sidebar/sidebar.ui")]
    pub struct EpistleSidebar {
        #[template_child]
        pub(super) sidebar: TemplateChild<adw::Sidebar>,
        #[template_child]
        pub(super) menu_btn: TemplateChild<gtk::MenuButton>,

        pub(super) accounts: std::cell::OnceCell<Arc<dyn MailAccounts>>,
        pub(super) folders: std::cell::OnceCell<Arc<dyn MailFolders>>,
        pub(super) sender: std::cell::OnceCell<EventSender>,
        /// Maps (email_address, display_name) → (account_id, folder_name)
        pub(super) folder_map: RefCell<HashMap<(String, String), (String, String)>>,
    }

    impl std::fmt::Debug for EpistleSidebar {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("EpistleSidebar").finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpistleSidebar {
        const NAME: &'static str = "EpistleSidebar";
        type Type = super::EpistleSidebar;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EpistleSidebar {
        fn constructed(&self) {
            self.parent_constructed();

            // Build the primary menu in code — can't load from window.ui
            // because it's a CompositeTemplate (Builder rejects templates).
            let menu = gtk::gio::Menu::new();
            let section = gtk::gio::Menu::new();
            section.append(Some("_Preferences"), Some("app.preferences"));
            section.append(Some("_Keyboard Shortcuts"), Some("app.shortcuts"));
            section.append(Some("_About Epistle"), Some("app.about"));
            menu.append_section(None, &section);
            self.menu_btn.set_menu_model(Some(&menu));

            // Add the default "Unified" section
            self.sidebar.append(Self::build_unified_section());
        }
    }

    impl WidgetImpl for EpistleSidebar {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            obj.subscribe_events();
            obj.wire_selection();
            obj.load_cached();
        }

        fn unroot(&self) {
            self.parent_unroot();
        }
    }

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

    /// Inject engine trait objects. Must be called before the widget is rooted.
    pub fn set_engine(
        &self,
        accounts: Arc<dyn MailAccounts>,
        folders: Arc<dyn MailFolders>,
        sender: EventSender,
    ) {
        let imp = self.imp();
        imp.accounts.set(accounts).ok().expect("accounts set once");
        imp.folders.set(folders).ok().expect("folders set once");
        imp.sender.set(sender).ok().expect("sender set once");
    }

    fn subscribe_events(&self) {
        let sidebar_weak = self.downgrade();
        epistle::event_bus::subscribe(move |event| {
            let Some(sidebar) = sidebar_weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::AccountsChanged { accounts } => {
                    sidebar.on_accounts_changed(accounts);
                }
                AppEvent::FoldersChanged {
                    account_id,
                    email_address,
                    folders,
                } => {
                    sidebar.on_folders_changed(account_id, email_address, folders);
                }
                _ => {}
            }
        });
    }

    fn load_cached(&self) {
        let accounts = Arc::clone(self.imp().accounts.get().expect("engine set before root"));
        let folders = Arc::clone(self.imp().folders.get().expect("engine set before root"));

        let sidebar_weak = self.downgrade();
        glib::MainContext::default().spawn_local(async move {
            let Some(sidebar) = sidebar_weak.upgrade() else {
                return;
            };
            let Ok(cached_accounts) = accounts.list_accounts().await else {
                return;
            };
            if cached_accounts.is_empty() {
                return;
            }
            sidebar.on_accounts_changed(&cached_accounts);
            for account in &cached_accounts {
                if let Ok(cached_folders) = folders.list_folders(&account.goa_id).await {
                    if !cached_folders.is_empty() {
                        sidebar.on_folders_changed(
                            &account.goa_id,
                            &account.email_address,
                            &cached_folders,
                        );
                    }
                }
            }
        });
    }

    fn on_accounts_changed(&self, accounts: &[Account]) {
        let sidebar = &*self.imp().sidebar;

        // Build set of existing section titles to avoid duplicates
        let existing: std::collections::HashSet<String> = (0..sidebar.sections().n_items())
            .filter_map(|i| sidebar.section(i)?.title().map(|t| t.to_string()))
            .collect();

        for account in accounts {
            if existing.contains(&account.email_address) {
                continue;
            }
            let section = adw::SidebarSection::new();
            section.set_title(Some(&account.email_address));
            sidebar.append(section);
        }
    }

    fn on_folders_changed(&self, account_id: &str, email_address: &str, folders: &[Folder]) {
        let imp = self.imp();
        let sidebar = &*imp.sidebar;

        let sections = sidebar.sections();
        for i in 0..sections.n_items() {
            let Some(section) = sidebar.section(i) else {
                continue;
            };
            if section.title().as_deref() == Some(email_address) {
                section.remove_all();
                let mut map = imp.folder_map.borrow_mut();
                for folder in folders {
                    let icon = icon_for_role(folder.role.as_deref());
                    let display_name =
                        display_name_for_folder(&folder.name, folder.role.as_deref());
                    map.insert(
                        (email_address.to_string(), display_name.clone()),
                        (account_id.to_string(), folder.name.clone()),
                    );
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

    fn wire_selection(&self) {
        let imp = self.imp();
        let sidebar_widget = imp.sidebar.clone();
        let weak = self.downgrade();
        sidebar_widget.connect_notify_local(Some("selected-item"), move |sidebar, _| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let Some(item) = sidebar.selected_item() else {
                return;
            };
            let Some(section) = item.section() else {
                return;
            };
            let Some(section_title) = section.title() else {
                return;
            };
            let Some(item_title) = item.title().map(|t| t.to_string()) else {
                return;
            };
            let imp = this.imp();
            let map = imp.folder_map.borrow();
            let key = (section_title.to_string(), item_title);
            if let Some((account_id, folder_name)) = map.get(&key) {
                if let Some(sender) = imp.sender.get() {
                    sender.send(AppEvent::FolderSelected {
                        account_id: account_id.clone(),
                        folder_name: folder_name.clone(),
                    });
                }
            }
        });
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
