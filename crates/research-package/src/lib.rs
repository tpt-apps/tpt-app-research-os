//! Self-contained project packages: a zip of the project directory, a manifest
//! that lists every file with its SHA-256 and size, an Ed25519 signature over the
//! manifest, and optional passphrase encryption. Format is described in
//! `docs/PACKAGE_FORMAT.md`.
//!
//! Import checks, in order: the signature, the format version, every entry name,
//! the declared total size, that the archive holds nothing the manifest does not
//! list, then each file's length and hash. Nothing is written to disk until all of
//! that passes.

mod crypto;
mod error;

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use research_core::ProjectManifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub use crypto::{decrypt, encrypt, is_encrypted};
pub use error::{PackageError, Result};

/// Version of the package container layout.
pub const FORMAT_VERSION: u32 = 1;

/// Largest total uncompressed size accepted on import. Stops zip bombs.
pub const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The project manifest, as stored inside the project directory and the package.
pub const PROJECT_MANIFEST: &str = "manifest.json";

const PACKAGE_MANIFEST: &str = "package.json";
const SIGNATURE: &str = "package.sig";
/// Written by `research-store` to lock the project; never packed.
const LOCK_FILE: &str = ".lock";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PATH_LEN: usize = 512;
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The signed table of contents of a package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub format_version: u32,
    /// Id of the project this package holds. Must match its `manifest.json`.
    pub project_id: String,
    /// Creation time, ISO-8601 text as supplied by the caller.
    pub created: String,
    /// The signer's Ed25519 public key, 64 hex digits.
    pub signer: String,
    pub entries: Vec<Entry>,
}

/// One file listed in a package manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// A verified package: its project manifest and every file, as bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub project: ProjectManifest,
    /// Relative path (with `/`) and contents, sorted by path. Includes `manifest.json`.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Generate a new Ed25519 signing key from the operating system's random source.
pub fn generate_signing_key() -> Result<SigningKey> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|_| PackageError::Random)?;
    Ok(SigningKey::from_bytes(&seed))
}

/// Pack a closed project directory into signed package bytes.
///
/// Fails if another handle holds the project open, because its files may be
/// changing while they are read. Symbolic links are refused.
pub fn pack_project(dir: &Path, created: &str, signer: &SigningKey) -> Result<Vec<u8>> {
    let manifest_text = std::fs::read_to_string(dir.join(PROJECT_MANIFEST))
        .map_err(|_| PackageError::Missing(PROJECT_MANIFEST.into()))?;
    let project = ProjectManifest::from_json(&manifest_text)?;

    let _lock = lock_project(dir)?;
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    pack_files(&files, project.id.as_str(), created, signer)
}

/// Pack in-memory files. Paths are relative with `/` separators and must include
/// `manifest.json`. Used by [`pack_project`]; exposed for callers that build
/// packages from other sources.
pub fn pack_files(
    files: &[(String, Vec<u8>)],
    project_id: &str,
    created: &str,
    signer: &SigningKey,
) -> Result<Vec<u8>> {
    let mut files = files.to_vec();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    check_file_set(&files)?;
    if !files.iter().any(|(p, _)| p == PROJECT_MANIFEST) {
        return Err(PackageError::Missing(PROJECT_MANIFEST.into()));
    }

    let manifest = PackageManifest {
        format_version: FORMAT_VERSION,
        project_id: project_id.to_string(),
        created: created.to_string(),
        signer: hex(signer.verifying_key().as_bytes()),
        entries: files
            .iter()
            .map(|(path, data)| Entry {
                path: path.clone(),
                sha256: hex(&Sha256::digest(data)),
                size: data.len() as u64,
            })
            .collect(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let signature = signer.sign(&manifest_bytes);
    build_zip(&manifest_bytes, &signature.to_bytes(), &files)
}

/// The public key that signed `bytes`, as its manifest records it.
///
/// This does not verify anything. Callers compare the key with one they trust,
/// then call [`verify`] or [`unpack_project`] with it.
pub fn signer_key(bytes: &[u8]) -> Result<VerifyingKey> {
    let mut archive = open_archive(bytes)?;
    let manifest_bytes = read_bounded(&mut archive, PACKAGE_MANIFEST, MAX_MANIFEST_BYTES)?
        .ok_or_else(|| PackageError::Missing(PACKAGE_MANIFEST.into()))?;
    let manifest: PackageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| PackageError::Manifest(e.to_string()))?;
    let raw: [u8; 32] = decode_hex(&manifest.signer)
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| PackageError::Manifest("signer key is not 64 hex digits".into()))?;
    VerifyingKey::from_bytes(&raw)
        .map_err(|_| PackageError::Manifest("signer key is invalid".into()))
}

/// Verify a package and return its contents. Nothing is written to disk.
pub fn verify(bytes: &[u8], verifier: &VerifyingKey) -> Result<Package> {
    let mut archive = open_archive(bytes)?;

    let manifest_bytes = read_bounded(&mut archive, PACKAGE_MANIFEST, MAX_MANIFEST_BYTES)?
        .ok_or_else(|| PackageError::Missing(PACKAGE_MANIFEST.into()))?;
    let signature_bytes = read_bounded(&mut archive, SIGNATURE, 64)?
        .ok_or_else(|| PackageError::Missing(SIGNATURE.into()))?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| PackageError::BadSignature)?;
    verifier
        .verify(&manifest_bytes, &signature)
        .map_err(|_| PackageError::BadSignature)?;

    let manifest: PackageManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| PackageError::Manifest(e.to_string()))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(PackageError::UnsupportedFormat {
            found: manifest.format_version,
            supported: FORMAT_VERSION,
        });
    }
    check_listed_paths(&manifest.entries)?;

    let mut total: u64 = 0;
    for entry in &manifest.entries {
        total = total.saturating_add(entry.size);
    }
    if total > MAX_UNPACKED_BYTES {
        return Err(PackageError::TooLarge {
            limit: MAX_UNPACKED_BYTES,
        });
    }

    let listed: HashSet<&str> = manifest.entries.iter().map(|e| e.path.as_str()).collect();
    for name in archive_names(&archive) {
        if name != PACKAGE_MANIFEST && name != SIGNATURE && !listed.contains(name.as_str()) {
            return Err(PackageError::Unlisted(name));
        }
    }

    let mut files = BTreeMap::new();
    for entry in &manifest.entries {
        // Read at most the declared size plus one byte, so a lying archive
        // cannot make us buffer more than it promised.
        let data = read_bounded(&mut archive, &entry.path, entry.size)?
            .ok_or_else(|| PackageError::Missing(entry.path.clone()))?;
        if data.len() as u64 != entry.size || hex(&Sha256::digest(&data)) != entry.sha256 {
            return Err(PackageError::HashMismatch {
                path: entry.path.clone(),
            });
        }
        files.insert(entry.path.clone(), data);
    }

    let project_bytes = files
        .get(PROJECT_MANIFEST)
        .ok_or_else(|| PackageError::Missing(PROJECT_MANIFEST.into()))?;
    let project_text =
        std::str::from_utf8(project_bytes).map_err(|e| PackageError::Manifest(e.to_string()))?;
    let project = ProjectManifest::from_json(project_text)?;
    if project.id.as_str() != manifest.project_id {
        return Err(PackageError::Manifest(
            "project id in package manifest does not match manifest.json".into(),
        ));
    }

    Ok(Package {
        project,
        files: files.into_iter().collect(),
    })
}

/// Verify a package and write its project to `dest`, which must not exist.
///
/// Files are written to a `.partial` sibling first and renamed into place, so a
/// failure never leaves a half-written project at `dest`. Returns the project
/// manifest.
pub fn unpack_project(
    bytes: &[u8],
    verifier: &VerifyingKey,
    dest: &Path,
) -> Result<ProjectManifest> {
    if dest.exists() {
        return Err(PackageError::DestinationExists(dest.to_path_buf()));
    }
    let package = verify(bytes, verifier)?;

    let staging = staging_path(dest)?;
    if staging.exists() {
        return Err(PackageError::DestinationExists(staging));
    }
    let result = write_tree(&staging, &package.files)
        .and_then(|()| std::fs::rename(&staging, dest).map_err(PackageError::from));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result?;
    Ok(package.project)
}

fn write_tree(root: &Path, files: &[(String, Vec<u8>)]) -> Result<()> {
    for (path, data) in files {
        let target = path
            .split('/')
            .fold(root.to_path_buf(), |acc, part| acc.join(part));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, data)?;
    }
    Ok(())
}

fn staging_path(dest: &Path) -> Result<PathBuf> {
    let name = dest
        .file_name()
        .ok_or_else(|| PackageError::UnsafePath(dest.display().to_string()))?;
    let mut staged = name.to_os_string();
    staged.push(".partial");
    Ok(dest.with_file_name(staged))
}

/// Take an exclusive lock on the project directory, the same lock the store takes.
fn lock_project(dir: &Path) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(PackageError::ProjectInUse(dir.to_path_buf()))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(PackageError::Symlink(path));
        }
        if kind.is_dir() {
            collect_files(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("walked paths are under the root");
        if relative == Path::new(LOCK_FILE) {
            continue;
        }
        let name = relative
            .components()
            .map(|c| c.as_os_str().to_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| PackageError::UnsafePath(relative.display().to_string()))?
            .join("/");
        out.push((name, std::fs::read(&path)?));
    }
    Ok(())
}

/// Checks on the files being packed: safe names, no reserved names, no
/// case-insensitive collisions.
fn check_file_set(files: &[(String, Vec<u8>)]) -> Result<()> {
    let mut seen = HashSet::new();
    for (path, _) in files {
        check_safe_path(path)?;
        if path == PACKAGE_MANIFEST || path == SIGNATURE {
            return Err(PackageError::UnsafePath(path.clone()));
        }
        if !seen.insert(path.to_lowercase()) {
            return Err(PackageError::Duplicate(path.clone()));
        }
    }
    Ok(())
}

/// Checks on the entry list of a manifest read on import.
fn check_listed_paths(entries: &[Entry]) -> Result<()> {
    let mut seen = HashSet::new();
    for entry in entries {
        check_safe_path(&entry.path)?;
        if entry.path == PACKAGE_MANIFEST || entry.path == SIGNATURE {
            return Err(PackageError::UnsafePath(entry.path.clone()));
        }
        if !seen.insert(entry.path.to_lowercase()) {
            return Err(PackageError::Duplicate(entry.path.clone()));
        }
    }
    Ok(())
}

/// Accept only relative, portable paths: no absolute paths, no `..`, no empty or
/// dot components, no backslashes or drive letters, no characters Windows forbids,
/// and no Windows reserved device names.
fn check_safe_path(path: &str) -> Result<()> {
    let bad = || PackageError::UnsafePath(path.to_string());
    if path.is_empty() || path.len() > MAX_PATH_LEN || path.starts_with('/') {
        return Err(bad());
    }
    if path
        .chars()
        .any(|c| matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Err(bad());
    }
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with('.')
            || component.ends_with(' ')
        {
            return Err(bad());
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if WINDOWS_RESERVED.contains(&stem.as_str()) {
            return Err(bad());
        }
    }
    Ok(())
}

fn build_zip(manifest: &[u8], signature: &[u8], files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    write_entry(&mut writer, PACKAGE_MANIFEST, manifest, options)?;
    write_entry(&mut writer, SIGNATURE, signature, options)?;
    for (path, data) in files {
        write_entry(&mut writer, path, data, options)?;
    }
    let cursor = writer
        .finish()
        .map_err(|e| PackageError::Archive(e.to_string()))?;
    Ok(cursor.into_inner())
}

fn write_entry(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    path: &str,
    data: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    writer
        .start_file(path, options)
        .map_err(|e| PackageError::Archive(e.to_string()))?;
    writer.write_all(data)?;
    Ok(())
}

fn open_archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>> {
    ZipArchive::new(Cursor::new(bytes)).map_err(|e| PackageError::Archive(e.to_string()))
}

/// Every name in the central directory, duplicates included.
fn archive_names(archive: &ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
    archive.file_names().map(str::to_string).collect()
}

/// Read one entry, failing if it holds more than `limit` bytes. `None` if absent.
fn read_bounded(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    path: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>> {
    let mut file = match archive.by_name(path) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(PackageError::Archive(e.to_string())),
    };
    let mut data = Vec::new();
    file.by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(PackageError::HashMismatch {
            path: path.to_string(),
        });
    }
    Ok(Some(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::{ProjectId, ResultId, RunId};
    use research_store::{ProjectStore, Run, StoredResult, Value};

    fn signer() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// A closed project with one run and one result, at `dir`.
    fn make_project(dir: &Path) {
        let manifest =
            ProjectManifest::new(ProjectId::new("study-1").unwrap(), "Study", 1).unwrap();
        let project = ProjectStore::create(dir, &manifest).unwrap();
        let run_id = RunId::new("run-a").unwrap();
        project
            .save_run(&Run {
                id: run_id.clone(),
                label: "baseline".into(),
                inputs: BTreeMap::from([("k".to_string(), "2.5".to_string())]),
                code_hash: "h1".into(),
                environment: BTreeMap::new(),
                started_unix: 10,
                finished_unix: 20,
            })
            .unwrap();
        project
            .save_result(&StoredResult {
                id: ResultId::new("res-1").unwrap(),
                run_id,
                name: "energy".into(),
                value: Value::Scalar { value: 4.2 },
            })
            .unwrap();
    }

    fn simple_files() -> Vec<(String, Vec<u8>)> {
        let project = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        vec![
            (
                PROJECT_MANIFEST.to_string(),
                project.to_json().unwrap().into_bytes(),
            ),
            ("data/input.csv".to_string(), b"t,x\n0,1\n".to_vec()),
        ]
    }

    /// Build a validly signed package from raw parts, bypassing `pack_files`'s
    /// checks. Used to test that import rejects bad packages on its own.
    fn forge(
        entries: &[(&str, &[u8])],
        declared_size: Option<u64>,
        format_version: u32,
    ) -> Vec<u8> {
        let files: Vec<(String, Vec<u8>)> = entries
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_vec()))
            .collect();
        let manifest = PackageManifest {
            format_version,
            project_id: "p".into(),
            created: "now".into(),
            signer: hex(signer().verifying_key().as_bytes()),
            entries: files
                .iter()
                .map(|(path, data)| Entry {
                    path: path.clone(),
                    sha256: hex(&Sha256::digest(data)),
                    size: declared_size.unwrap_or(data.len() as u64),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        let sig = signer().sign(&bytes).to_bytes();
        build_zip(&bytes, &sig, &files).unwrap()
    }

    fn with_entry_replaced(bytes: &[u8], path: &str, data: &[u8]) -> Vec<u8> {
        let mut archive = open_archive(bytes).unwrap();
        let mut entries = Vec::new();
        for name in archive_names(&archive) {
            let body = read_bounded(&mut archive, &name, u64::MAX / 2)
                .unwrap()
                .unwrap();
            entries.push((name, body));
        }
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, body) in entries {
            out.start_file(&name, options).unwrap();
            let body = if name == path { data.to_vec() } else { body };
            out.write_all(&body).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn project_round_trips_to_another_directory() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("source");
        make_project(&src);

        let packed = pack_project(&src, "2026-10-14T00:00:00Z", &signer()).unwrap();
        let dest = work.path().join("elsewhere");
        let project = unpack_project(&packed, &signer().verifying_key(), &dest).unwrap();
        assert_eq!(project.name, "Study");

        let copy = ProjectStore::open(&dest).unwrap();
        assert_eq!(copy.manifest().id.as_str(), "study-1");
        let run = copy
            .load_run(&RunId::new("run-a").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(run.label, "baseline");
        let results = copy.results_for_run(&run.id).unwrap();
        assert_eq!(results[0].value, Value::Scalar { value: 4.2 });
        assert!(!work.path().join("elsewhere.partial").exists());
    }

    #[test]
    fn packing_an_open_project_is_refused() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("source");
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "P", 0).unwrap();
        let _held = ProjectStore::create(&src, &manifest).unwrap();
        assert!(matches!(
            pack_project(&src, "t", &signer()),
            Err(PackageError::ProjectInUse(_))
        ));
    }

    #[test]
    fn unpacking_refuses_an_existing_destination() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("source");
        make_project(&src);
        let packed = pack_project(&src, "t", &signer()).unwrap();
        assert!(matches!(
            unpack_project(&packed, &signer().verifying_key(), &src),
            Err(PackageError::DestinationExists(_))
        ));
    }

    #[test]
    fn wrong_signer_is_rejected() {
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let packed = pack_files(&simple_files(), "p", "t", &signer()).unwrap();
        assert!(matches!(
            verify(&packed, &other.verifying_key()),
            Err(PackageError::BadSignature)
        ));
    }

    #[test]
    fn signer_key_names_the_signer_without_trusting_it() {
        let packed = pack_files(&simple_files(), "p", "t", &signer()).unwrap();
        assert_eq!(signer_key(&packed).unwrap(), signer().verifying_key());
    }

    #[test]
    fn altered_file_content_is_detected() {
        let packed = pack_files(&simple_files(), "p", "t", &signer()).unwrap();
        let tampered = with_entry_replaced(&packed, "data/input.csv", b"t,x\n0,999\n");
        assert!(matches!(
            verify(&tampered, &signer().verifying_key()),
            Err(PackageError::HashMismatch { .. })
        ));
    }

    #[test]
    fn archive_entry_the_manifest_does_not_list_is_rejected() {
        let files = simple_files();
        let extra = b"sneaked in".to_vec();
        let packed = pack_files(&files, "p", "t", &signer()).unwrap();
        let mut archive = open_archive(&packed).unwrap();
        let mut names: Vec<(String, Vec<u8>)> = archive_names(&archive)
            .into_iter()
            .map(|n| {
                let body = read_bounded(&mut archive, &n, u64::MAX / 2)
                    .unwrap()
                    .unwrap();
                (n, body)
            })
            .collect();
        names.push(("extra.txt".into(), extra));
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        for (n, body) in &names {
            out.start_file(n, SimpleFileOptions::default()).unwrap();
            out.write_all(body).unwrap();
        }
        let tampered = out.finish().unwrap().into_inner();
        assert!(matches!(
            verify(&tampered, &signer().verifying_key()),
            Err(PackageError::Unlisted(name)) if name == "extra.txt"
        ));
    }

    #[test]
    fn manifest_listing_a_path_twice_is_rejected() {
        // The zip writer refuses duplicate names, so an archive with two entries of
        // one name cannot be built here. The manifest-level check still applies.
        let entry = Entry {
            path: "a.txt".into(),
            sha256: String::new(),
            size: 0,
        };
        assert!(matches!(
            check_listed_paths(&[entry.clone(), entry]),
            Err(PackageError::Duplicate(_))
        ));
    }

    #[test]
    fn declared_size_larger_than_the_data_is_rejected() {
        let files = [(PROJECT_MANIFEST, &b"{}"[..]), ("a.txt", &b"abc"[..])];
        let forged = forge(&files, Some(10), FORMAT_VERSION);
        assert!(verify(&forged, &signer().verifying_key()).is_err());
    }

    #[test]
    fn oversized_declared_total_is_refused_before_reading() {
        let files = [(PROJECT_MANIFEST, &b"{}"[..]), ("a.txt", &b"abc"[..])];
        let forged = forge(&files, Some(MAX_UNPACKED_BYTES), FORMAT_VERSION);
        assert!(matches!(
            verify(&forged, &signer().verifying_key()),
            Err(PackageError::TooLarge { .. })
        ));
    }

    #[test]
    fn newer_format_version_is_refused() {
        let files = [(PROJECT_MANIFEST, &b"{}"[..])];
        let forged = forge(&files, None, FORMAT_VERSION + 1);
        assert!(matches!(
            verify(&forged, &signer().verifying_key()),
            Err(PackageError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn traversal_and_unsafe_names_are_rejected_on_import() {
        for bad in [
            "../escape.txt",
            "/abs.txt",
            "a/../b.txt",
            "a\\b.txt",
            "C:x.txt",
            "CON.txt",
            "trail.",
            "a//b",
        ] {
            let files = [(PROJECT_MANIFEST, &b"{}"[..]), (bad, &b"x"[..])];
            let forged = forge(&files, None, FORMAT_VERSION);
            assert!(
                matches!(
                    verify(&forged, &signer().verifying_key()),
                    Err(PackageError::UnsafePath(_))
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn case_insensitive_collisions_are_rejected_on_import() {
        let files = [
            (PROJECT_MANIFEST, &b"{}"[..]),
            ("Data.txt", &b"1"[..]),
            ("data.txt", &b"2"[..]),
        ];
        let forged = forge(&files, None, FORMAT_VERSION);
        assert!(matches!(
            verify(&forged, &signer().verifying_key()),
            Err(PackageError::Duplicate(_))
        ));
    }

    #[test]
    fn packing_rejects_reserved_and_colliding_names() {
        let mut files = simple_files();
        files.push(("package.json".into(), b"x".to_vec()));
        assert!(matches!(
            pack_files(&files, "p", "t", &signer()),
            Err(PackageError::UnsafePath(_))
        ));

        let mut files = simple_files();
        files.push(("DATA/INPUT.CSV".into(), b"x".to_vec()));
        assert!(matches!(
            pack_files(&files, "p", "t", &signer()),
            Err(PackageError::Duplicate(_))
        ));
    }

    #[test]
    fn packing_requires_the_project_manifest() {
        let files = vec![("a.txt".to_string(), b"x".to_vec())];
        assert!(matches!(
            pack_files(&files, "p", "t", &signer()),
            Err(PackageError::Missing(_))
        ));
    }

    #[test]
    fn project_id_must_match_the_package_manifest() {
        let files = simple_files();
        // The packed manifest claims a different id from the one in manifest.json.
        let packed = pack_files(&files, "not-p", "t", &signer()).unwrap();
        assert!(matches!(
            verify(&packed, &signer().verifying_key()),
            Err(PackageError::Manifest(_))
        ));
    }

    #[test]
    fn encrypted_package_needs_the_passphrase() {
        let work = tempfile::tempdir().unwrap();
        let src = work.path().join("source");
        make_project(&src);
        let packed = pack_project(&src, "t", &signer()).unwrap();
        let sealed = encrypt(&packed, "pw").unwrap();

        let opened = decrypt(&sealed, Some("pw")).unwrap();
        let project = verify(&opened, &signer().verifying_key()).unwrap();
        assert_eq!(project.project.id.as_str(), "study-1");
        assert!(decrypt(&sealed, None).is_err());
    }

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        assert_eq!(decode_hex(&hex(&[0, 255, 16])).unwrap(), vec![0, 255, 16]);
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
    }
}
