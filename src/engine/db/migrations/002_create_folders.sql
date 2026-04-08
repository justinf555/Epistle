CREATE TABLE folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id  TEXT NOT NULL REFERENCES accounts(goa_id),
    name        TEXT NOT NULL,
    delimiter   TEXT,
    role        TEXT,
    uidvalidity INTEGER,
    uidnext     INTEGER,
    UNIQUE(account_id, name)
);
