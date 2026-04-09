//! Filesystem-based storage for raw email bodies (.eml files).
//!
//! Bodies are stored in a two-level hash-sharded directory tree:
//! `{base_dir}/{account_id}/{xx}/{yy}/{uuid}.eml`
//! where `xx` and `yy` are the first two and next two hex characters of
//! the UUID. This prevents filesystem performance degradation on large
//! mailboxes (most filesystems slow down past ~50k entries per directory).
//!
//! Writes are atomic via a tmp+rename pattern to prevent partial reads
//! if the app crashes mid-write.

use std::path::PathBuf;

use tokio::fs;

/// Manages raw email body storage on the filesystem.
#[derive(Debug, Clone)]
pub struct BodyStore {
    base_dir: PathBuf,
}

impl BodyStore {
    /// Create a new BodyStore rooted at the given directory.
    /// Creates the directory if it doesn't exist.
    pub async fn open(base_dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&base_dir).await?;
        Ok(Self { base_dir })
    }

    /// Write raw RFC 5322 bytes atomically into the sharded directory tree.
    pub async fn store_eml(&self, account_id: &str, uuid: &str, raw: &[u8]) -> std::io::Result<()> {
        let shard_dir = self.shard_dir(account_id, uuid);
        fs::create_dir_all(&shard_dir).await?;

        let final_path = self.eml_path(account_id, uuid);
        let tmp_path = shard_dir.join(format!("{uuid}.eml.tmp"));

        fs::write(&tmp_path, raw).await?;
        fs::rename(&tmp_path, &final_path).await?;

        tracing::debug!(account_id, uuid, bytes = raw.len(), "Stored .eml file");
        Ok(())
    }

    /// Read raw RFC 5322 bytes for a message. Returns None if file doesn't exist.
    pub async fn read_eml(&self, account_id: &str, uuid: &str) -> std::io::Result<Option<Vec<u8>>> {
        let path = self.eml_path(account_id, uuid);
        match fs::read(&path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check whether a .eml file exists for the given message.
    pub async fn has_eml(&self, account_id: &str, uuid: &str) -> bool {
        self.eml_path(account_id, uuid).exists()
    }

    /// Full path: `{base_dir}/{account_id}/{xx}/{yy}/{uuid}.eml`
    pub fn eml_path(&self, account_id: &str, uuid: &str) -> PathBuf {
        self.shard_dir(account_id, uuid).join(format!("{uuid}.eml"))
    }

    /// Shard directory: `{base_dir}/{account_id}/{xx}/{yy}/`
    /// Uses first two and next two hex characters of the UUID.
    fn shard_dir(&self, account_id: &str, uuid: &str) -> PathBuf {
        let hex = uuid.replace('-', "");
        let xx = &hex[..2];
        let yy = &hex[2..4];
        self.base_dir.join(account_id).join(xx).join(yy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn store_and_read_eml() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("messages")).await.unwrap();

        let account = "acct1";
        let uuid = "a1b2c3d4-e5f6-7890-abcd-1234567890ef";
        let raw = b"From: alice@example.com\r\nSubject: Test\r\n\r\nHello!";

        store.store_eml(account, uuid, raw).await.unwrap();
        assert!(store.has_eml(account, uuid).await);

        let data = store.read_eml(account, uuid).await.unwrap().unwrap();
        assert_eq!(data, raw);

        // Verify sharded path
        let path = store.eml_path(account, uuid);
        assert!(path.to_str().unwrap().contains("a1/b2"));
    }

    #[tokio::test]
    async fn read_missing_returns_none() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("messages")).await.unwrap();

        let data = store.read_eml("acct1", "nonexistent-uuid").await.unwrap();
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn separate_accounts_separate_dirs() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("messages")).await.unwrap();

        let uuid = "a1b2c3d4-e5f6-7890-abcd-1234567890ef";
        store.store_eml("acct1", uuid, b"body1").await.unwrap();
        store.store_eml("acct2", uuid, b"body2").await.unwrap();

        let d1 = store.read_eml("acct1", uuid).await.unwrap().unwrap();
        let d2 = store.read_eml("acct2", uuid).await.unwrap().unwrap();
        assert_eq!(d1, b"body1");
        assert_eq!(d2, b"body2");
    }
}
