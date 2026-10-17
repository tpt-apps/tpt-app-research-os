//! Opening and creating a project directory, and the schema behind it.
//!
//! A project directory holds:
//! - `manifest.json`: the [`ProjectManifest`], written atomically;
//! - `objects/` and `local/`: the Keystone object store and database.

use std::fs::File;
use std::fs::TryLockError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use research_core::ProjectManifest;
use tpt_keystone::executor::execute_query;
use tpt_keystone::storage::config::{NodeRole, UdfConfig};
use tpt_keystone::storage::database::Database;
use tpt_keystone::storage::lease::LeaseManager;
use tpt_keystone::storage::objectstore::{LocalFsObjectStore, ObjectStore};

use crate::error::{Result, StoreError};

/// Schema version this build writes and understands.
pub const SCHEMA_VERSION: u32 = 2;

const MANIFEST_FILE: &str = "manifest.json";
const LOCK_FILE: &str = ".lock";
const SCHEMA_KEY: &str = "schema_version";
/// Directory of content-addressed blobs: one file per SHA-256.
pub(crate) const BLOB_DIR: &str = "blobs";

const RUNS_TABLE: &str = "CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, doc TEXT)";
const RESULTS_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS results (id TEXT PRIMARY KEY, run_id TEXT, doc TEXT)";
/// Added in schema version 2.
const DOCUMENTS_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS documents (id TEXT PRIMARY KEY, title TEXT, doc TEXT)";

/// Table DDL for the current schema, in creation order.
const TABLES: &[&str] = &[RUNS_TABLE, RESULTS_TABLE, DOCUMENTS_TABLE];

/// An open research project.
///
/// Holding a `ProjectStore` holds the project lock. A second `open` of the same
/// directory fails until this one is dropped.
pub struct ProjectStore {
    pub(crate) dir: PathBuf,
    manifest: ProjectManifest,
    pub(crate) db: Arc<Database>,
    // Last field so it is released after `db` has closed. Held for the life of the handle.
    _lock: File,
}

impl ProjectStore {
    /// Create a new project in `dir` with the given manifest.
    ///
    /// Fails if `dir` already contains a project. The directory is created if
    /// needed.
    pub fn create(dir: &Path, manifest: &ProjectManifest) -> Result<Self> {
        if dir.join(MANIFEST_FILE).exists() {
            return Err(StoreError::AlreadyExists(dir.to_path_buf()));
        }
        std::fs::create_dir_all(dir)?;
        write_atomic(&dir.join(MANIFEST_FILE), manifest.to_json()?.as_bytes())?;
        Self::open_db(dir, manifest.clone())
    }

    /// Open an existing project in `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        let json = match std::fs::read_to_string(dir.join(MANIFEST_FILE)) {
            Ok(json) => json,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::ProjectMissing(dir.to_path_buf()));
            }
            Err(e) => return Err(e.into()),
        };
        let manifest = ProjectManifest::from_json(&json)?;
        Self::open_db(dir, manifest)
    }

    /// The project's manifest as it was last written.
    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    /// The project directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn open_db(dir: &Path, manifest: ProjectManifest) -> Result<Self> {
        let lock = acquire_lock(dir)?;
        std::fs::create_dir_all(dir.join(BLOB_DIR))?;
        let bucket = dir.join("objects");
        std::fs::create_dir_all(&bucket)?;
        let store: Arc<dyn ObjectStore> =
            Arc::new(LocalFsObjectStore::open(&bucket).map_err(db_err)?);
        let lease = Arc::new(LeaseManager::new(
            store.clone(),
            "research",
            "local".into(),
            Duration::from_secs(30),
        ));
        lease.try_acquire().map_err(db_err)?;
        let db = Database::open(
            &dir.join("local"),
            store,
            lease.handle(),
            NodeRole::Writer,
            UdfConfig::default(),
        )
        .map_err(db_err)?;
        let project = Self {
            dir: dir.to_path_buf(),
            manifest,
            db: Arc::new(db),
            _lock: lock,
        };
        project.migrate()?;
        Ok(project)
    }

    /// Bring the schema up to [`SCHEMA_VERSION`], or refuse if it is newer.
    fn migrate(&self) -> Result<()> {
        self.exec(
            "CREATE TABLE IF NOT EXISTS meta (setting TEXT PRIMARY KEY, setting_value TEXT)",
        )?;
        match self.stored_schema_version()? {
            None => {
                for sql in TABLES {
                    self.exec(sql)?;
                }
                self.write_schema_version(SCHEMA_VERSION)
            }
            Some(found) if found > SCHEMA_VERSION => Err(StoreError::SchemaTooNew {
                found,
                supported: SCHEMA_VERSION,
            }),
            Some(found) => {
                for from in found..SCHEMA_VERSION {
                    self.apply_migration(from)?;
                }
                if found < SCHEMA_VERSION {
                    self.write_schema_version(SCHEMA_VERSION)?;
                }
                Ok(())
            }
        }
    }

    /// Steps from schema `from` to `from + 1`. Add an arm here whenever
    /// `SCHEMA_VERSION` is bumped.
    fn apply_migration(&self, from: u32) -> Result<()> {
        match from {
            // 1 -> 2: documents table.
            1 => self.exec(DOCUMENTS_TABLE),
            _ => Err(StoreError::Corrupt(format!(
                "no migration from schema version {from}"
            ))),
        }
    }

    fn stored_schema_version(&self) -> Result<Option<u32>> {
        let rows = self.rows(&format!(
            "SELECT setting_value FROM meta WHERE setting = {}",
            sql_text(SCHEMA_KEY)
        ))?;
        match rows.first().and_then(|row| row.first()) {
            None => Ok(None),
            Some(text) => text.parse().map(Some).map_err(|_| {
                StoreError::Corrupt(format!("schema version {text:?} is not a number"))
            }),
        }
    }

    fn write_schema_version(&self, version: u32) -> Result<()> {
        self.exec(&format!(
            "DELETE FROM meta WHERE setting = {}",
            sql_text(SCHEMA_KEY)
        ))?;
        self.exec(&format!(
            "INSERT INTO meta (setting, setting_value) VALUES ({}, {})",
            sql_text(SCHEMA_KEY),
            sql_text(&version.to_string())
        ))
    }

    /// A setting the application keeps with the project, or `None` if it is unset.
    /// Keys must start with `app.`, so the store's own entries cannot be changed.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        check_setting_key(key)?;
        let rows = self.rows(&format!(
            "SELECT setting_value FROM meta WHERE setting = {}",
            sql_text(key)
        ))?;
        Ok(rows.first().and_then(|row| row.first()).cloned())
    }

    /// Set an application setting, replacing any earlier value. Same key rules as
    /// [`ProjectStore::setting`].
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        check_setting_key(key)?;
        self.exec(&format!(
            "DELETE FROM meta WHERE setting = {}",
            sql_text(key)
        ))?;
        self.exec(&format!(
            "INSERT INTO meta (setting, setting_value) VALUES ({}, {})",
            sql_text(key),
            sql_text(value)
        ))
    }

    /// Run a statement that returns no rows.
    pub(crate) fn exec(&self, sql: &str) -> Result<()> {
        execute_query(sql, self.db.clone()).map_err(db_err)?;
        Ok(())
    }

    /// Run a query and return every row as text cells. Null cells become "".
    pub(crate) fn rows(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let result = execute_query(sql, self.db.clone()).map_err(db_err)?;
        Ok(result
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| {
                        cell.as_deref()
                            .map(|b| String::from_utf8_lossy(b).into_owned())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect())
    }
}

/// Take an exclusive OS lock on the project directory. The OS drops it if the
/// process dies, so a crash never leaves the project locked.
fn acquire_lock(dir: &Path) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(StoreError::Locked(dir.to_path_buf())),
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Write `bytes` to `path` via a temporary file and a rename, so a crash never
/// leaves a half-written file behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Quote a value as a SQL string literal, doubling embedded single quotes.
/// Application setting keys: `app.` followed by letters, digits, `.`, `-` or `_`.
fn check_setting_key(key: &str) -> Result<()> {
    let body = key.strip_prefix("app.").unwrap_or("");
    let valid = !body.is_empty()
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(StoreError::InvalidSettingKey(key.to_string()))
    }
}

pub(crate) fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn db_err(e: anyhow::Error) -> StoreError {
    StoreError::Database(format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::ProjectId;

    fn manifest(name: &str) -> ProjectManifest {
        ProjectManifest::new(ProjectId::new("proj-1").unwrap(), name, 1_700_000_000).unwrap()
    }

    #[test]
    fn app_settings_persist_and_the_schema_key_is_protected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        {
            let project = ProjectStore::create(&root, &manifest("S")).unwrap();
            assert_eq!(project.setting("app.agents").unwrap(), None);
            project.set_setting("app.agents", "on").unwrap();
            project.set_setting("app.agents", "off").unwrap();
        }
        let project = ProjectStore::open(&root).unwrap();
        assert_eq!(
            project.setting("app.agents").unwrap().as_deref(),
            Some("off")
        );
        assert!(matches!(
            project.set_setting("schema_version", "99"),
            Err(StoreError::InvalidSettingKey(_))
        ));
        assert!(matches!(
            project.setting("app."),
            Err(StoreError::InvalidSettingKey(_))
        ));
        // The schema version is untouched, so the project still opens.
        drop(project);
        assert!(ProjectStore::open(&root).is_ok());
    }

    #[test]
    fn create_then_reopen_keeps_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        {
            let project = ProjectStore::create(&root, &manifest("First")).unwrap();
            assert_eq!(project.manifest().name, "First");
        }
        let reopened = ProjectStore::open(&root).unwrap();
        assert_eq!(reopened.manifest().name, "First");
        assert_eq!(reopened.dir(), root);
    }

    #[test]
    fn create_refuses_an_existing_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        ProjectStore::create(&root, &manifest("A")).unwrap();
        let second = ProjectStore::create(&root, &manifest("B"));
        assert!(matches!(second, Err(StoreError::AlreadyExists(_))));
    }

    #[test]
    fn open_missing_project_errors() {
        let dir = tempfile::tempdir().unwrap();
        let r = ProjectStore::open(&dir.path().join("nothing-here"));
        assert!(matches!(r, Err(StoreError::ProjectMissing(_))));
    }

    #[test]
    fn second_open_while_held_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        let _held = ProjectStore::create(&root, &manifest("A")).unwrap();
        let second = ProjectStore::open(&root);
        assert!(matches!(second, Err(StoreError::Locked(_))));
    }

    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        ProjectStore::create(&root, &manifest("A")).unwrap();
        assert!(root.join(MANIFEST_FILE).exists());
        assert!(!root.join("manifest.json.tmp").exists());
    }

    #[test]
    fn newer_schema_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        {
            let project = ProjectStore::create(&root, &manifest("A")).unwrap();
            project.write_schema_version(SCHEMA_VERSION + 1).unwrap();
        }
        let r = ProjectStore::open(&root);
        assert!(matches!(
            r,
            Err(StoreError::SchemaTooNew { found, supported })
                if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
        ));
    }

    #[test]
    fn version_one_store_is_upgraded_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        {
            let project = ProjectStore::create(&root, &manifest("A")).unwrap();
            project.write_schema_version(1).unwrap();
        }
        let project = ProjectStore::open(&root).unwrap();
        assert_eq!(
            project.stored_schema_version().unwrap(),
            Some(SCHEMA_VERSION)
        );
    }
}
