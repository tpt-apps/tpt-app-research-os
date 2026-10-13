//! Content-addressed binary storage: plots, images and data files.
//!
//! A blob is stored as `blobs/<sha256-hex>` in the project directory. Identical
//! content is stored once. The file name is the hash, so a blob cannot be
//! addressed by anything but a valid hash, and every read re-checks the hash.

use sha2::{Digest, Sha256};

use crate::error::{Result, StoreError};
use crate::project::{ProjectStore, BLOB_DIR};

/// Largest single blob accepted. Stops one write from exhausting memory.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024 * 1024;

impl ProjectStore {
    /// Store `data` and return its SHA-256 as 64 lowercase hex digits. Storing the
    /// same bytes twice is a no-op that returns the same hash.
    pub fn put_blob(&self, data: &[u8]) -> Result<String> {
        if data.len() > MAX_BLOB_BYTES {
            return Err(StoreError::Invalid {
                what: "blob",
                reason: format!("larger than {MAX_BLOB_BYTES} bytes"),
            });
        }
        let hash = hex(&Sha256::digest(data));
        let target = self.blob_path(&hash)?;
        if target.exists() {
            return Ok(hash);
        }
        // Write beside the target and rename, so a crash never leaves a short blob
        // under its final name. A leftover .tmp is overwritten next time.
        let tmp = target.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(&tmp, &target)?;
        Ok(hash)
    }

    /// The bytes of a blob, or `None` if it is not stored. Fails if the stored
    /// bytes no longer match their hash.
    pub fn get_blob(&self, sha256: &str) -> Result<Option<Vec<u8>>> {
        let path = self.blob_path(sha256)?;
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if hex(&Sha256::digest(&data)) != sha256 {
            return Err(StoreError::Corrupt(format!(
                "blob {sha256} does not match its hash"
            )));
        }
        Ok(Some(data))
    }

    /// True if a blob with this hash is stored.
    pub fn has_blob(&self, sha256: &str) -> Result<bool> {
        Ok(self.blob_path(sha256)?.exists())
    }

    /// The path for a hash, after checking the hash is well formed. Only valid
    /// hashes reach the file system, so a crafted value cannot name another file.
    fn blob_path(&self, sha256: &str) -> Result<std::path::PathBuf> {
        if !is_sha256(sha256) {
            return Err(StoreError::Invalid {
                what: "blob hash",
                reason: format!("{sha256:?} is not 64 lowercase hex digits"),
            });
        }
        Ok(self.dir.join(BLOB_DIR).join(sha256))
    }
}

/// True for exactly 64 lowercase hex digits.
pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::ProjectId;
    use research_core::ProjectManifest;

    fn open_temp() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        let project = ProjectStore::create(&dir.path().join("p"), &manifest).unwrap();
        (dir, project)
    }

    #[test]
    fn blob_round_trips_and_is_content_addressed() {
        let (_dir, project) = open_temp();
        let a = project.put_blob(b"<svg/>").unwrap();
        let b = project.put_blob(b"<svg/>").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_eq!(project.get_blob(&a).unwrap().unwrap(), b"<svg/>");
        assert!(project.has_blob(&a).unwrap());
        assert_eq!(
            project.get_blob(&hex(&Sha256::digest(b"other"))).unwrap(),
            None
        );
    }

    #[test]
    fn blobs_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        let hash = {
            let project = ProjectStore::create(&root, &manifest).unwrap();
            project.put_blob(b"plot bytes").unwrap()
        };
        let project = ProjectStore::open(&root).unwrap();
        assert_eq!(project.get_blob(&hash).unwrap().unwrap(), b"plot bytes");
    }

    #[test]
    fn malformed_hashes_never_reach_the_file_system() {
        let (_dir, project) = open_temp();
        for bad in [
            "../manifest",
            "ABCDEF",
            "",
            &"a".repeat(63),
            &"A".repeat(64),
            &format!("{}/x", "a".repeat(62)),
        ] {
            assert!(
                matches!(project.get_blob(bad), Err(StoreError::Invalid { .. })),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn altered_blob_is_detected_on_read() {
        let (_dir, project) = open_temp();
        let hash = project.put_blob(b"original").unwrap();
        std::fs::write(project.dir.join(BLOB_DIR).join(&hash), b"changed").unwrap();
        assert!(matches!(
            project.get_blob(&hash),
            Err(StoreError::Corrupt(_))
        ));
    }
}
