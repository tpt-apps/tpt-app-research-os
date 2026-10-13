//! Project manifest schema. Stored as `manifest.json` at the root of a project
//! directory and inside exported packages.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::id::ProjectId;

/// Current manifest format version. Bump when the schema changes in a way
/// older builds cannot read.
pub const FORMAT_VERSION: u32 = 1;

/// Top-level description of a research project.
///
/// `BTreeMap` is used for `foundation_revs` so the serialized output is
/// deterministic and diff-friendly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    /// Schema version this manifest was written with.
    pub format_version: u32,
    /// Stable identifier of the project.
    pub id: ProjectId,
    /// Human-readable project name. Must not be blank.
    pub name: String,
    /// Creation time as seconds since the Unix epoch (UTC).
    pub created_unix: u64,
    /// Git revisions of the foundation crates the project was last written with,
    /// keyed by crate name. Recorded so results can be tied to exact dependencies.
    #[serde(default)]
    pub foundation_revs: BTreeMap<String, String>,
}

impl ProjectManifest {
    /// Creates a manifest at the current [`FORMAT_VERSION`].
    pub fn new(id: ProjectId, name: impl Into<String>, created_unix: u64) -> Result<Self> {
        let manifest = Self {
            format_version: FORMAT_VERSION,
            id,
            name: name.into(),
            created_unix,
            foundation_revs: BTreeMap::new(),
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Checks invariants that serde alone does not enforce.
    pub fn validate(&self) -> Result<()> {
        if self.format_version > FORMAT_VERSION {
            return Err(Error::UnsupportedFormatVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }
        if self.name.trim().is_empty() {
            return Err(Error::InvalidManifest("name is blank".into()));
        }
        if let Some(crate_name) = self
            .foundation_revs
            .iter()
            .find_map(|(k, v)| (k.trim().is_empty() || v.trim().is_empty()).then_some(k.as_str()))
        {
            return Err(Error::InvalidManifest(format!(
                "foundation rev for {crate_name:?} is blank"
            )));
        }
        Ok(())
    }

    /// Parses and validates a manifest from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Serializes the manifest as pretty-printed JSON with a trailing newline.
    pub fn to_json(&self) -> Result<String> {
        self.validate()?;
        let mut out = serde_json::to_string_pretty(self)?;
        out.push('\n');
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ProjectManifest {
        let mut m = ProjectManifest::new(
            ProjectId::new("proj-1").unwrap(),
            "Spring study",
            1_700_000_000,
        )
        .unwrap();
        m.foundation_revs.insert("tpt-math".into(), "abc123".into());
        m
    }

    #[test]
    fn round_trip_is_stable() {
        let m = sample();
        let json = m.to_json().unwrap();
        let back = ProjectManifest::from_json(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.to_json().unwrap(), json);
    }

    #[test]
    fn rejects_blank_name() {
        let r = ProjectManifest::new(ProjectId::new("p").unwrap(), "   ", 0);
        assert!(matches!(r, Err(Error::InvalidManifest(_))));
    }

    #[test]
    fn rejects_newer_format_version() {
        let mut m = sample();
        m.format_version = FORMAT_VERSION + 1;
        let json = serde_json::to_string(&m).unwrap();
        assert!(matches!(
            ProjectManifest::from_json(&json),
            Err(Error::UnsupportedFormatVersion { .. })
        ));
    }

    #[test]
    fn rejects_invalid_project_id_in_json() {
        let json = r#"{"format_version":1,"id":"../x","name":"n","created_unix":0}"#;
        assert!(matches!(
            ProjectManifest::from_json(json),
            Err(Error::Serialization(_))
        ));
    }

    #[test]
    fn rejects_blank_foundation_rev() {
        let mut m = sample();
        m.foundation_revs.insert("tpt-math".into(), "  ".into());
        assert!(m.validate().is_err());
    }
}
