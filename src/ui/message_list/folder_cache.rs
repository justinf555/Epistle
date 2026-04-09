use std::cell::RefCell;
use std::collections::HashMap;

use gtk::gio;

use super::item::MessageObject;

/// Maximum number of folder models to keep cached.
const MAX_CACHED_FOLDERS: usize = 10;

/// Number of messages to load per page.
pub const PAGE_SIZE: u32 = 200;

type FolderKey = (String, String); // (account_id, folder_name)

/// Per-folder cached state: the ListStore, UID index, pagination, and scroll position.
pub struct FolderEntry {
    pub store: gio::ListStore,
    pub uid_index: HashMap<u32, u32>,
    pub loaded_count: u32,
    pub all_loaded: bool,
    pub loading: bool,
    pub scroll_position: f64,
}

impl FolderEntry {
    fn new() -> Self {
        Self {
            store: gio::ListStore::new::<MessageObject>(),
            uid_index: HashMap::new(),
            loaded_count: 0,
            all_loaded: false,
            loading: false,
            scroll_position: 0.0,
        }
    }
}

/// LRU cache of per-folder ListStores.
///
/// Keeps up to `MAX_CACHED_FOLDERS` folder models in memory. Switching
/// folders swaps the model on the SingleSelection — no clearing or
/// reloading. Sync events update any folder's cache, even if not displayed.
pub struct FolderModelCache {
    entries: RefCell<HashMap<FolderKey, FolderEntry>>,
    /// Access order for LRU eviction (most recent at the end).
    order: RefCell<Vec<FolderKey>>,
}

impl Default for FolderModelCache {
    fn default() -> Self {
        Self {
            entries: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
        }
    }
}

impl FolderModelCache {
    /// Get an existing folder entry, promoting it to MRU.
    /// Returns None if the folder hasn't been cached yet.
    pub fn get(&self, account_id: &str, folder_name: &str) -> bool {
        let key = (account_id.to_string(), folder_name.to_string());
        let exists = self.entries.borrow().contains_key(&key);
        if exists {
            self.promote(&key);
        }
        exists
    }

    /// Get or create a folder entry, promoting it to MRU.
    /// Returns true if the entry already existed (cache hit).
    pub fn get_or_create(&self, account_id: &str, folder_name: &str) -> bool {
        let key = (account_id.to_string(), folder_name.to_string());
        let existed = self.entries.borrow().contains_key(&key);

        if existed {
            self.promote(&key);
        } else {
            self.evict_if_full();
            self.entries.borrow_mut().insert(key.clone(), FolderEntry::new());
            self.order.borrow_mut().push(key);
        }

        existed
    }

    /// Try to access a folder entry if it exists.
    pub fn with_entry_if_exists<F, R>(
        &self,
        account_id: &str,
        folder_name: &str,
        f: F,
    ) -> Option<R>
    where
        F: FnOnce(&mut FolderEntry) -> R,
    {
        let key = (account_id.to_string(), folder_name.to_string());
        let mut entries = self.entries.borrow_mut();
        entries.get_mut(&key).map(f)
    }

    /// Get the ListStore for a folder (for setting on the selection model).
    pub fn store_for(&self, account_id: &str, folder_name: &str) -> gio::ListStore {
        let key = (account_id.to_string(), folder_name.to_string());
        self.entries
            .borrow()
            .get(&key)
            .expect("folder entry must exist")
            .store
            .clone()
    }

    fn promote(&self, key: &FolderKey) {
        let mut order = self.order.borrow_mut();
        if let Some(pos) = order.iter().position(|k| k == key) {
            order.remove(pos);
        }
        order.push(key.clone());
    }

    fn evict_if_full(&self) {
        let mut order = self.order.borrow_mut();
        let mut entries = self.entries.borrow_mut();

        while entries.len() >= MAX_CACHED_FOLDERS {
            if let Some(evicted_key) = order.first().cloned() {
                order.remove(0);
                entries.remove(&evicted_key);
                tracing::debug!(
                    account = %evicted_key.0,
                    folder = %evicted_key.1,
                    "Evicted folder from model cache"
                );
            } else {
                break;
            }
        }
    }
}
