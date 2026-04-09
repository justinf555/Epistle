use std::cell::RefCell;
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use epistle::app_event::AppEvent;
use epistle::engine::traits::accounts::MailAccounts;
use epistle::engine::traits::messages::{MailMessages, Message};
use epistle::event_bus::EventSender;

use super::factory;
use super::folder_cache::{FolderModelCache, PAGE_SIZE};
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
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,

        pub(super) selection: std::cell::OnceCell<gtk::SingleSelection>,
        pub(super) accounts: std::cell::OnceCell<Arc<dyn MailAccounts>>,
        pub(super) messages: std::cell::OnceCell<Arc<dyn MailMessages>>,
        pub(super) sender: std::cell::OnceCell<EventSender>,
        pub(super) current_account: RefCell<String>,
        pub(super) current_folder: RefCell<String>,
        pub(super) cache: FolderModelCache,
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

            // Create the selection model with an empty store.
            // The store is swapped when the user selects a folder.
            let store = gio::ListStore::new::<MessageObject>();
            let selection = gtk::SingleSelection::new(Some(store));
            self.list_view.set_model(Some(&selection));
            self.list_view.set_factory(Some(&factory::build_factory()));
            self.selection
                .set(selection)
                .expect("selection set once in constructed");
        }
    }

    impl WidgetImpl for EpistleMessageList {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();
            obj.subscribe_events();
            obj.wire_selection();
            obj.wire_scroll_pagination();
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
    pub fn set_engine(
        &self,
        accounts: Arc<dyn MailAccounts>,
        messages: Arc<dyn MailMessages>,
        sender: EventSender,
    ) {
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
        self.imp()
            .sender
            .set(sender)
            .ok()
            .expect("sender set once");
    }

    /// Access the sidebar toggle button for wiring to the outer split view.
    pub fn sidebar_toggle(&self) -> &gtk::ToggleButton {
        &self.imp().sidebar_toggle
    }

    /// Access the selection model for wiring selection changes.
    pub fn selection_model(&self) -> &gtk::SingleSelection {
        self.imp()
            .selection
            .get()
            .expect("selection initialized")
    }

    fn subscribe_events(&self) {
        let weak = self.downgrade();
        epistle::event_bus::subscribe(move |event| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            match event {
                AppEvent::FolderSelected {
                    account_id,
                    folder_name,
                } => {
                    list.show_folder(account_id, folder_name);
                }
                AppEvent::MessagesAdded {
                    account_id,
                    folder_name,
                    messages,
                } => {
                    list.on_messages_added_for(account_id, folder_name, messages);
                }
                AppEvent::MessagesUpdated {
                    account_id,
                    folder_name,
                    messages,
                } => {
                    list.on_messages_updated_for(account_id, folder_name, messages);
                }
                AppEvent::MessagesRemoved {
                    account_id,
                    folder_name,
                    uids,
                } => {
                    list.on_messages_removed_for(account_id, folder_name, uids);
                }
                _ => {}
            }
        });
    }

    fn is_current_folder(&self, account_id: &str, folder_name: &str) -> bool {
        let imp = self.imp();
        *imp.current_account.borrow() == account_id
            && *imp.current_folder.borrow() == folder_name
    }

    fn show_folder(&self, account_id: &str, folder_name: &str) {
        let imp = self.imp();

        // No-op if already showing this folder
        if self.is_current_folder(account_id, folder_name) {
            return;
        }

        // Save scroll position for the folder we're leaving
        self.save_scroll_position();

        // Update current folder tracking
        *imp.current_account.borrow_mut() = account_id.to_string();
        *imp.current_folder.borrow_mut() = folder_name.to_string();

        // Get or create cache entry — returns true if it was a cache hit
        let cache_hit = imp.cache.get_or_create(account_id, folder_name);

        // Swap the model on the selection
        let store = imp.cache.store_for(account_id, folder_name);
        self.selection_model().set_model(Some(&store));

        if store.n_items() > 0 {
            imp.stack.set_visible_child_name("list");
            // Restore scroll position after the model swap settles
            self.restore_scroll_position(account_id, folder_name);
        } else if cache_hit {
            // Cache hit but empty — folder genuinely has no messages
            imp.stack.set_visible_child_name("empty");
        } else {
            // Cache miss — need to load from DB. Show loading state.
            imp.stack.set_visible_child_name("empty");
            self.load_next_page();
        }
    }

    fn save_scroll_position(&self) {
        let imp = self.imp();
        let account_id = imp.current_account.borrow().clone();
        let folder_name = imp.current_folder.borrow().clone();
        if account_id.is_empty() {
            return;
        }
        let pos = imp.scrolled_window.vadjustment().value();
        imp.cache.with_entry_if_exists(&account_id, &folder_name, |entry| {
            entry.scroll_position = pos;
        });
    }

    fn restore_scroll_position(&self, account_id: &str, folder_name: &str) {
        let imp = self.imp();
        let pos = imp
            .cache
            .with_entry_if_exists(account_id, folder_name, |entry| entry.scroll_position)
            .unwrap_or(0.0);

        if pos > 0.0 {
            // Defer to next idle so the ListView has laid out the new model
            let sw = imp.scrolled_window.clone();
            glib::idle_add_local_once(move || {
                sw.vadjustment().set_value(pos);
            });
        }
    }

    fn load_next_page(&self) {
        let imp = self.imp();
        let account_id = imp.current_account.borrow().clone();
        let folder_name = imp.current_folder.borrow().clone();
        if account_id.is_empty() {
            return;
        }

        let can_load = imp
            .cache
            .with_entry_if_exists(&account_id, &folder_name, |entry| {
                if entry.all_loaded || entry.loading {
                    return None;
                }
                entry.loading = true;
                Some(entry.loaded_count)
            })
            .flatten();

        let Some(offset) = can_load else {
            return;
        };

        let messages = Arc::clone(imp.messages.get().expect("engine set before root"));
        let account_id_owned = account_id.clone();
        let folder_name_owned = folder_name.clone();

        let weak = self.downgrade();
        glib::MainContext::default().spawn_local(async move {
            let Some(list) = weak.upgrade() else {
                return;
            };
            // Verify folder hasn't changed while loading
            if !list.is_current_folder(&account_id_owned, &folder_name_owned) {
                // Reset loading flag on the entry we were loading for
                list.imp().cache.with_entry_if_exists(
                    &account_id_owned,
                    &folder_name_owned,
                    |entry| {
                        entry.loading = false;
                    },
                );
                return;
            }
            if let Ok(page) = messages
                .list_messages_page(&account_id_owned, &folder_name_owned, PAGE_SIZE, offset)
                .await
            {
                let count = page.len() as u32;
                if !page.is_empty() {
                    list.insert_messages_into_cache(
                        &account_id_owned,
                        &folder_name_owned,
                        &page,
                    );
                }
                list.imp().cache.with_entry_if_exists(
                    &account_id_owned,
                    &folder_name_owned,
                    |entry| {
                        entry.loaded_count = offset + count;
                        entry.all_loaded = count < PAGE_SIZE;
                        entry.loading = false;
                    },
                );
                // Update stack visibility if this is still the current folder
                if list.is_current_folder(&account_id_owned, &folder_name_owned) {
                    let store = list.imp().cache.store_for(&account_id_owned, &folder_name_owned);
                    if store.n_items() > 0 {
                        list.imp().stack.set_visible_child_name("list");
                    } else {
                        list.imp().stack.set_visible_child_name("empty");
                    }
                }
            } else {
                list.imp().cache.with_entry_if_exists(
                    &account_id_owned,
                    &folder_name_owned,
                    |entry| {
                        entry.loading = false;
                    },
                );
            }
        });
    }

    fn wire_scroll_pagination(&self) {
        let vadj = self.imp().scrolled_window.vadjustment();
        let weak = self.downgrade();
        vadj.connect_value_changed(move |adj| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            // Load more when within 500px of the bottom
            let remaining = adj.upper() - adj.page_size() - adj.value();
            if remaining < 500.0 {
                list.load_next_page();
            }
        });
    }

    fn wire_selection(&self) {
        let selection = self.selection_model().clone();
        let weak = self.downgrade();
        selection.connect_selection_changed(move |sel, _, _| {
            let Some(list) = weak.upgrade() else {
                return;
            };
            if let Some(item) = sel.selected_item().and_downcast::<MessageObject>() {
                if let Some(sender) = list.imp().sender.get() {
                    sender.send(AppEvent::MessageSelected {
                        account_id: item.account_id().unwrap_or_default(),
                        folder_name: item.folder_name().unwrap_or_default(),
                        uid: item.uid(),
                        subject: item.subject(),
                        sender: item.sender(),
                        date: item.date(),
                    });
                }
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

        let weak = self.downgrade();
        glib::MainContext::default().spawn_local(async move {
            let Some(list) = weak.upgrade() else {
                return;
            };

            let Ok(cached_accounts) = accounts.list_accounts().await else {
                return;
            };

            // Default to first account's INBOX
            if let Some(account) = cached_accounts.first() {
                list.show_folder(&account.goa_id, "INBOX");
            }
        });
    }

    // ── Cache-aware message operations ─────────────────────────────────

    /// Insert messages into a folder's cached store (bulk or incremental).
    fn insert_messages_into_cache(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[Message],
    ) {
        self.imp()
            .cache
            .with_entry_if_exists(account_id, folder_name, |entry| {
                if entry.store.n_items() == 0 {
                    // Bulk load: store is empty, messages are pre-sorted from DB.
                    let items: Vec<MessageObject> =
                        messages.iter().map(MessageObject::new).collect();
                    for (i, item) in items.iter().enumerate() {
                        entry.uid_index.insert(item.uid(), i as u32);
                    }
                    entry.store.splice(0, 0, &items);
                } else {
                    for msg in messages {
                        if entry.uid_index.contains_key(&msg.uid) {
                            continue; // Already in store, skip duplicate
                        }
                        let item = MessageObject::new(msg);
                        let uid = item.uid();
                        let pos = find_insert_position(&entry.store, item.sort_timestamp());

                        for val in entry.uid_index.values_mut() {
                            if *val >= pos {
                                *val += 1;
                            }
                        }
                        entry.store.insert(pos, &item);
                        entry.uid_index.insert(uid, pos);
                    }
                }
            });
    }

    /// Handle MessagesAdded for any folder (not just current).
    fn on_messages_added_for(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[Message],
    ) {
        // Only update folders that are already cached
        if self.imp().cache.get(account_id, folder_name) {
            self.insert_messages_into_cache(account_id, folder_name, messages);

            // Update stack if this is the current folder
            if self.is_current_folder(account_id, folder_name) {
                let store = self.imp().cache.store_for(account_id, folder_name);
                if store.n_items() > 0 {
                    self.imp().stack.set_visible_child_name("list");
                }

                // Auto-scroll to top if user is already near the top
                let vadj = self.imp().scrolled_window.vadjustment();
                if vadj.value() < 50.0 {
                    vadj.set_value(0.0);
                }
            }
        }
    }

    /// Handle MessagesUpdated for any folder (not just current).
    fn on_messages_updated_for(
        &self,
        account_id: &str,
        folder_name: &str,
        messages: &[Message],
    ) {
        self.imp()
            .cache
            .with_entry_if_exists(account_id, folder_name, |entry| {
                for msg in messages {
                    if let Some(&pos) = entry.uid_index.get(&msg.uid) {
                        if let Some(obj) = entry.store.item(pos).and_downcast::<MessageObject>() {
                            obj.update_from(msg);
                        }
                    }
                }
            });
    }

    /// Handle MessagesRemoved for any folder (not just current).
    fn on_messages_removed_for(&self, account_id: &str, folder_name: &str, uids: &[u32]) {
        self.imp()
            .cache
            .with_entry_if_exists(account_id, folder_name, |entry| {
                let mut positions: Vec<u32> = uids
                    .iter()
                    .filter_map(|uid| entry.uid_index.remove(uid))
                    .collect();
                positions.sort_unstable_by(|a, b| b.cmp(a));

                for pos in &positions {
                    entry.store.remove(*pos);
                }

                // Rebuild index after removals
                if !positions.is_empty() {
                    entry.uid_index.clear();
                    for i in 0..entry.store.n_items() {
                        if let Some(obj) = entry.store.item(i).and_downcast::<MessageObject>() {
                            entry.uid_index.insert(obj.uid(), i);
                        }
                    }
                }
            });

        // Update stack if current folder is now empty
        if self.is_current_folder(account_id, folder_name) {
            let store = self.imp().cache.store_for(account_id, folder_name);
            if store.n_items() == 0 {
                self.imp().stack.set_visible_child_name("empty");
            }
        }
    }
}

/// Binary search for the insertion point in a newest-first sorted store.
/// Uses pre-computed Unix timestamps for correct ordering regardless of date format.
fn find_insert_position(store: &gio::ListStore, timestamp: i64) -> u32 {
    let n = store.n_items();
    if n == 0 {
        return 0;
    }

    let mut low: u32 = 0;
    let mut high: u32 = n;

    while low < high {
        let mid = low + (high - low) / 2;
        let mid_obj = store
            .item(mid)
            .and_downcast::<MessageObject>()
            .expect("ListStore contains only MessageObject");
        let mid_ts = mid_obj.sort_timestamp();

        if timestamp >= mid_ts {
            high = mid;
        } else {
            low = mid + 1;
        }
    }

    low
}
