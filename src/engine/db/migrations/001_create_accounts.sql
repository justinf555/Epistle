CREATE TABLE accounts (
    goa_id        TEXT PRIMARY KEY,
    provider_type TEXT NOT NULL,
    email_address TEXT NOT NULL,
    display_name  TEXT,
    imap_host     TEXT NOT NULL,
    imap_port     INTEGER NOT NULL,
    imap_tls_mode TEXT NOT NULL,
    smtp_host     TEXT,
    smtp_port     INTEGER,
    smtp_tls_mode TEXT,
    active        INTEGER NOT NULL DEFAULT 1,
    last_sync     TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
