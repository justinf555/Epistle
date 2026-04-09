//! Filesystem-based storage for raw email bodies (.eml files).
//!
//! Bodies are written atomically via a tmp+rename pattern to prevent
//! partial reads if the app crashes mid-write. Files are stored as
//! `{uuid}.eml` in the data directory.

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

    /// Write raw RFC 5322 bytes atomically: {uuid}.eml.tmp → rename → {uuid}.eml
    pub async fn store_eml(&self, uuid: &str, raw: &[u8]) -> std::io::Result<()> {
        let final_path = self.eml_path(uuid);
        let tmp_path = self.base_dir.join(format!("{uuid}.eml.tmp"));

        fs::write(&tmp_path, raw).await?;
        fs::rename(&tmp_path, &final_path).await?;

        tracing::debug!(uuid, bytes = raw.len(), "Stored .eml file");
        Ok(())
    }

    /// Read raw RFC 5322 bytes for a message. Returns None if file doesn't exist.
    pub async fn read_eml(&self, uuid: &str) -> std::io::Result<Option<Vec<u8>>> {
        let path = self.eml_path(uuid);
        match fs::read(&path).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check whether a .eml file exists for the given UUID.
    pub async fn has_eml(&self, uuid: &str) -> bool {
        self.eml_path(uuid).exists()
    }

    /// Path to the .eml file for a given UUID.
    pub fn eml_path(&self, uuid: &str) -> PathBuf {
        self.base_dir.join(format!("{uuid}.eml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn store_and_read_eml() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("bodies")).await.unwrap();

        let uuid = "test-uuid-1234";
        let raw = b"From: alice@example.com\r\nSubject: Test\r\n\r\nHello!";

        store.store_eml(uuid, raw).await.unwrap();
        assert!(store.has_eml(uuid).await);

        let data = store.read_eml(uuid).await.unwrap().unwrap();
        assert_eq!(data, raw);
    }

    #[tokio::test]
    async fn read_missing_returns_none() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("bodies")).await.unwrap();

        let data = store.read_eml("nonexistent").await.unwrap();
        assert!(data.is_none());
    }

    #[tokio::test]
    async fn has_eml_false_when_missing() {
        let dir = tempdir().unwrap();
        let store = BodyStore::open(dir.path().join("bodies")).await.unwrap();

        assert!(!store.has_eml("nonexistent").await);
    }
}
