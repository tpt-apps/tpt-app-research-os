//! Errors for packing, verifying and unpacking.

use std::path::PathBuf;

/// Result alias using [`PackageError`].
pub type Result<T> = std::result::Result<T, PackageError>;

/// Errors produced by the package format.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    /// The bytes are not a readable zip archive.
    #[error("package is not a valid zip archive: {0}")]
    Archive(String),

    /// A required entry is absent.
    #[error("package is missing {0}")]
    Missing(String),

    /// The package manifest or project manifest is malformed.
    #[error("manifest is malformed: {0}")]
    Manifest(String),

    /// The signature does not verify against the expected key.
    #[error("package signature is invalid")]
    BadSignature,

    /// The package was written by a newer format version than this build reads.
    #[error("format version {found} is not supported (this build reads {supported})")]
    UnsupportedFormat { found: u32, supported: u32 },

    /// A file's bytes or length differ from its manifest entry.
    #[error("file `{path}` does not match its manifest entry (tampered or truncated)")]
    HashMismatch { path: String },

    /// The archive holds an entry the manifest does not list.
    #[error("archive contains `{0}`, which the manifest does not list")]
    Unlisted(String),

    /// The archive holds the same entry name more than once.
    #[error("archive lists `{0}` more than once")]
    Duplicate(String),

    /// The declared total size is larger than the import limit.
    #[error("package would unpack to more than {limit} bytes")]
    TooLarge { limit: u64 },

    /// An entry name could escape the project directory or is not portable.
    #[error("entry name `{0}` is not a safe relative path")]
    UnsafePath(String),

    /// Another handle has the project open, so its files may be changing.
    #[error("project at {0} is open; close it before packing")]
    ProjectInUse(PathBuf),

    /// The unpack destination already exists.
    #[error("{0} already exists")]
    DestinationExists(PathBuf),

    /// The project directory contains something that cannot be packed.
    #[error("cannot pack {0}: symbolic links are not allowed")]
    Symlink(PathBuf),

    /// An encrypted package was opened without a passphrase.
    #[error("encrypted package needs a passphrase")]
    PassphraseRequired,

    /// Wrong passphrase, or the encrypted bytes were altered.
    #[error("wrong passphrase or corrupted encrypted package")]
    Decrypt,

    /// Key derivation failed.
    #[error("key derivation failed")]
    Kdf,

    /// The operating system random source failed.
    #[error("random number generator failed")]
    Random,

    /// A shared type (id, project manifest) was invalid.
    #[error(transparent)]
    Core(#[from] research_core::Error),

    /// JSON could not be written.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A file could not be read or written.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
