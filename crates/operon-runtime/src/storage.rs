use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use serde_json::Value;

const STATE_TABLE: TableDefinition<&str, &str> = TableDefinition::new("state");

pub struct Storage {
    db: Database,
}

impl Storage {
    /// Open or create redb database
    pub fn open(path: &str) -> Result<Self> {
        let db = Database::create(path).context("Failed to create database")?;

        // Create table if not exists
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(STATE_TABLE)?;
        }
        write_txn.commit()?;

        Ok(Self { db })
    }

    /// Save state to database
    pub fn save_state(&self, key: &str, value: &Value) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(STATE_TABLE)?;
            let value_str = serde_json::to_string(value)?;
            table.insert(key, value_str.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Load state from database
    pub fn load_state(&self, key: &str) -> Result<Option<Value>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(STATE_TABLE)?;

        match table.get(key)? {
            Some(value) => {
                let value_str = value.value();
                let value: Value = serde_json::from_str(value_str)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// List all stored step keys
    pub fn list_keys(&self) -> Result<Vec<String>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(STATE_TABLE)?;
        let mut keys = Vec::new();
        for entry in table.iter()? {
            let (key, _): (redb::AccessGuard<&str>, redb::AccessGuard<&str>) = entry?;
            keys.push(key.value().to_string());
        }
        Ok(keys)
    }
}
