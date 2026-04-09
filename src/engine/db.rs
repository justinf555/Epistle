use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::SqlitePool;

use thiserror::Error;

pub mod accounts;
pub mod folders;
pub mod messages;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manages Epistle's SQLite database.
///
/// Wraps a [`SqlitePool`] configured with WAL mode for concurrent access
/// across the sync service (Tokio) and GTK command layer (GLib main loop).
///
/// Obtain via [`Database::open`], which creates the database file if needed
/// and runs all outstanding migrations before returning.
#[derive(Clone, Debug)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Open (or create) the database at `db_path`.
    ///
    /// Creates the parent directory if it does not exist, configures WAL mode,
    /// and runs all pending migrations before returning.
    pub async fn open(db_path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        sqlx::migrate!("src/engine/db/migrations")
            .run(&pool)
            .await?;

        Ok(Self { pool })
    }

    /// Access the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn open_creates_database_and_runs_migrations() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sub").join("mail.db");

        let db = Database::open(&db_path).await.unwrap();
        assert!(db_path.exists());

        // Verify accounts table was created
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='accounts'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(row.0, 1);
    }
}
