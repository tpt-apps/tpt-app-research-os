//! Typed identifiers. Each id is a validated, non-empty string made of ASCII
//! letters, digits, `-` and `_`, so it is safe to use as a file or directory name.

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Maximum length of an id, keeping paths well inside OS limits.
pub const MAX_ID_LEN: usize = 128;

/// An identifier tagged with the kind of entity it names.
///
/// The tag makes a `RunId` and a `DocId` distinct types, so they cannot be
/// passed to each other by mistake even though both are strings underneath.
pub struct Id<T> {
    value: String,
    _kind: PhantomData<fn() -> T>,
}

// Implemented by hand: `#[derive]` would require `T: Clone + Eq + ...`, which
// the marker types do not need to satisfy.
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _kind: PhantomData,
        }
    }
}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> Serialize for Id<T> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.value)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl<T> Id<T> {
    /// Validates `value` and wraps it as an id of kind `T`.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate(&value)?;
        Ok(Self {
            value,
            _kind: PhantomData,
        })
    }

    /// The underlying string form of the id.
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

fn validate(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::InvalidId("id is empty".into()));
    }
    if value.len() > MAX_ID_LEN {
        return Err(Error::InvalidId(format!(
            "id is longer than {MAX_ID_LEN} characters"
        )));
    }
    if let Some(bad) = value
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(Error::InvalidId(format!(
            "id {value:?} contains disallowed character {bad:?}"
        )));
    }
    Ok(())
}

impl<T> TryFrom<String> for Id<T> {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl<T> From<Id<T>> for String {
    fn from(id: Id<T>) -> String {
        id.value
    }
}

impl<T> FromStr for Id<T> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({:?})", self.value)
    }
}

/// Marker types for each id kind.
pub mod kind {
    /// A research project (one directory, one database).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Project {}
    /// One execution of a computation, recorded in the store.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Run {}
    /// A computational document (notebook-style).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Doc {}
    /// One stored result produced by a run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Result {}
    /// One cell of a computational document.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Cell {}
}

/// Identifies a research project.
pub type ProjectId = Id<kind::Project>;
/// Identifies a run recorded in the store.
pub type RunId = Id<kind::Run>;
/// Identifies a computational document.
pub type DocId = Id<kind::Doc>;
/// Identifies a result stored by a run.
pub type ResultId = Id<kind::Result>;
/// Identifies a cell within a document. Stable across saves and edits.
pub type CellId = Id<kind::Cell>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        assert!(ProjectId::new("my-project_01").is_ok());
        assert!(RunId::new("R20260101").is_ok());
    }

    #[test]
    fn rejects_empty_and_unsafe_ids() {
        assert!(DocId::new("").is_err());
        assert!(DocId::new("../escape").is_err());
        assert!(DocId::new("with space").is_err());
        assert!(DocId::new("nul\0byte").is_err());
        assert!(DocId::new("a".repeat(MAX_ID_LEN + 1)).is_err());
        assert!(DocId::new("a".repeat(MAX_ID_LEN)).is_ok());
    }

    #[test]
    fn serde_round_trip_validates_on_input() {
        let id = RunId::new("run-1").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"run-1\"");
        let back: RunId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        assert!(serde_json::from_str::<RunId>("\"bad/id\"").is_err());
    }

    #[test]
    fn display_and_parse() {
        let id: ResultId = "res_42".parse().unwrap();
        assert_eq!(id.to_string(), "res_42");
        assert_eq!(id.as_str(), "res_42");
    }
}
