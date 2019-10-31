//! # interledger-store-sqlite
//!
//! Data store for Interledger.rs using SQLite

mod store;

pub use store::{SqliteStore, SqliteStoreBuilder};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn open() {
        assert!(Connection::open_in_memory().is_ok());
    }
}
