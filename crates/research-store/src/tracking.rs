//! Experiment tracking: run records and the results they produce.
//!
//! Each run and each result is stored as one JSON document. Keystone holds the
//! documents; the id and run id columns are there so lookups do not have to
//! parse every row.

use std::collections::BTreeMap;

use research_core::{ResultId, RunId};
use serde::{Deserialize, Serialize};

use crate::blobs::is_sha256;
use crate::error::{Result, StoreError};
use crate::project::{sql_text, ProjectStore};

/// One execution of a computation: what went in, what code and environment ran it,
/// and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    /// Short human-readable name, e.g. "damped oscillator, k=2.5".
    pub label: String,
    /// Input parameters, as text so any kind of setting can be recorded.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
    /// Hash of the code that ran (e.g. the document revision). Opaque to the store.
    pub code_hash: String,
    /// Environment details, such as seed, solver version and host.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Start time, seconds since the Unix epoch.
    pub started_unix: u64,
    /// Finish time, seconds since the Unix epoch.
    pub finished_unix: u64,
}

impl Run {
    /// Check the fields that serde cannot.
    pub fn validate(&self) -> Result<()> {
        if self.label.trim().is_empty() {
            return Err(invalid("run", "label is blank"));
        }
        if self.code_hash.trim().is_empty() {
            return Err(invalid("run", "code hash is blank"));
        }
        if self.finished_unix < self.started_unix {
            return Err(invalid("run", "finished before it started"));
        }
        Ok(())
    }
}

/// A value produced by a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Value {
    Scalar {
        value: f64,
    },
    Array {
        values: Vec<f64>,
    },
    /// Each row has one value per column.
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<f64>>,
    },
    Text {
        text: String,
    },
    /// Binary content, such as a plot or image, held in the blob store. `sha256`
    /// names the blob; `media_type` is a MIME type such as `image/svg+xml`.
    Blob {
        media_type: String,
        sha256: String,
    },
}

impl Value {
    fn validate(&self) -> Result<()> {
        match self {
            Value::Scalar { value } => finite(*value),
            Value::Array { values } => values.iter().try_for_each(|v| finite(*v)),
            Value::Table { columns, rows } => {
                if columns.is_empty() {
                    return Err(invalid("result", "table has no columns"));
                }
                for (i, name) in columns.iter().enumerate() {
                    if name.trim().is_empty() || columns[..i].contains(name) {
                        return Err(invalid(
                            "result",
                            "column names must be unique and non-empty",
                        ));
                    }
                }
                for row in rows {
                    if row.len() != columns.len() {
                        return Err(invalid("result", "table row width does not match columns"));
                    }
                    row.iter().try_for_each(|v| finite(*v))?;
                }
                Ok(())
            }
            Value::Text { .. } => Ok(()),
            Value::Blob { media_type, sha256 } => {
                if media_type.trim().is_empty()
                    || media_type.len() > 128
                    || media_type.chars().any(char::is_control)
                {
                    return Err(invalid(
                        "result",
                        "blob media type is not a valid MIME type",
                    ));
                }
                if !is_sha256(sha256) {
                    return Err(invalid(
                        "result",
                        "blob hash is not 64 lowercase hex digits",
                    ));
                }
                Ok(())
            }
        }
    }
}

/// A named result of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredResult {
    pub id: ResultId,
    pub run_id: RunId,
    /// Name within the run, e.g. "energy" or "trajectory".
    pub name: String,
    pub value: Value,
}

impl StoredResult {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(invalid("result", "name is blank"));
        }
        self.value.validate()
    }
}

/// Runs side by side, from [`ProjectStore::compare_runs`].
#[derive(Debug, Clone, PartialEq)]
pub struct RunComparison {
    /// The runs, in the order asked for.
    pub runs: Vec<Run>,
    /// Input names whose value is not the same in every run (a missing input counts).
    pub differing_inputs: Vec<String>,
    /// For each result name, one entry per run: `None` where that run has no result
    /// of that name. Names are in alphabetical order.
    pub results: Vec<(String, Vec<Option<Value>>)>,
}

impl ProjectStore {
    /// Insert or replace a run.
    pub fn save_run(&self, run: &Run) -> Result<()> {
        run.validate()?;
        let id = sql_text(run.id.as_str());
        let doc = sql_text(&serde_json::to_string(run)?);
        self.exec(&format!("DELETE FROM runs WHERE id = {id}"))?;
        self.exec(&format!("INSERT INTO runs (id, doc) VALUES ({id}, {doc})"))
    }

    /// Load a run by id, or `None` if it is not stored.
    pub fn load_run(&self, id: &RunId) -> Result<Option<Run>> {
        let rows = self.rows(&format!(
            "SELECT doc FROM runs WHERE id = {}",
            sql_text(id.as_str())
        ))?;
        rows.first()
            .map(|row| parse_doc(row.first(), "run"))
            .transpose()
    }

    /// Every run, oldest first (by start time, then id).
    pub fn list_runs(&self) -> Result<Vec<Run>> {
        let mut runs = self.load_all_runs()?;
        runs.sort_by(|a, b| (a.started_unix, &a.id).cmp(&(b.started_unix, &b.id)));
        Ok(runs)
    }

    /// Runs for which `keep` returns true, oldest first.
    pub fn filter_runs(&self, keep: impl Fn(&Run) -> bool) -> Result<Vec<Run>> {
        Ok(self.list_runs()?.into_iter().filter(|r| keep(r)).collect())
    }

    /// Compare runs side by side: each run's inputs, the input names whose values
    /// differ between them, and each named result per run. Results are looked up by
    /// name, so a result missing from one run shows as `None` in that column.
    pub fn compare_runs(&self, ids: &[RunId]) -> Result<RunComparison> {
        let mut runs = Vec::with_capacity(ids.len());
        for id in ids {
            let run = self
                .load_run(id)?
                .ok_or_else(|| StoreError::RunNotFound(id.to_string()))?;
            runs.push(run);
        }

        let mut keys: Vec<String> = runs.iter().flat_map(|r| r.inputs.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        let differing_inputs = keys
            .into_iter()
            .filter(|key| {
                let first = runs.first().and_then(|r| r.inputs.get(key));
                runs.iter().any(|r| r.inputs.get(key) != first)
            })
            .collect();

        let mut result_names: Vec<String> = Vec::new();
        let mut per_run = Vec::with_capacity(runs.len());
        for run in &runs {
            let results = self.results_for_run(&run.id)?;
            for result in &results {
                if !result_names.contains(&result.name) {
                    result_names.push(result.name.clone());
                }
            }
            per_run.push(results);
        }
        result_names.sort();
        let results = result_names
            .into_iter()
            .map(|name| {
                let values = per_run
                    .iter()
                    .map(|results| {
                        results
                            .iter()
                            .find(|r| r.name == name)
                            .map(|r| r.value.clone())
                    })
                    .collect();
                (name, values)
            })
            .collect();

        Ok(RunComparison {
            runs,
            differing_inputs,
            results,
        })
    }

    /// Runs that were given `key` as an input with exactly `value`.
    pub fn runs_with_input(&self, key: &str, value: &str) -> Result<Vec<Run>> {
        self.filter_runs(|r| r.inputs.get(key).map(String::as_str) == Some(value))
    }

    /// Delete a run and every result it produced. Deleting a run that does not
    /// exist is not an error.
    pub fn delete_run(&self, id: &RunId) -> Result<()> {
        let id_sql = sql_text(id.as_str());
        self.exec(&format!("DELETE FROM results WHERE run_id = {id_sql}"))?;
        self.exec(&format!("DELETE FROM runs WHERE id = {id_sql}"))
    }

    /// Insert or replace a result. The run it belongs to must already be stored.
    pub fn save_result(&self, result: &StoredResult) -> Result<()> {
        result.validate()?;
        if self.load_run(&result.run_id)?.is_none() {
            return Err(StoreError::RunNotFound(result.run_id.to_string()));
        }
        if let Value::Blob { sha256, .. } = &result.value {
            if !self.has_blob(sha256)? {
                return Err(StoreError::BlobNotFound(sha256.clone()));
            }
        }
        let id = sql_text(result.id.as_str());
        let run_id = sql_text(result.run_id.as_str());
        let doc = sql_text(&serde_json::to_string(result)?);
        self.exec(&format!("DELETE FROM results WHERE id = {id}"))?;
        self.exec(&format!(
            "INSERT INTO results (id, run_id, doc) VALUES ({id}, {run_id}, {doc})"
        ))
    }

    /// The results of one run, ordered by name.
    pub fn results_for_run(&self, run_id: &RunId) -> Result<Vec<StoredResult>> {
        let rows = self.rows(&format!(
            "SELECT doc FROM results WHERE run_id = {}",
            sql_text(run_id.as_str())
        ))?;
        let mut results = rows
            .iter()
            .map(|row| parse_doc::<StoredResult>(row.first(), "result"))
            .collect::<Result<Vec<_>>>()?;
        results.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(results)
    }

    fn load_all_runs(&self) -> Result<Vec<Run>> {
        self.rows("SELECT doc FROM runs")?
            .iter()
            .map(|row| parse_doc(row.first(), "run"))
            .collect()
    }
}

fn parse_doc<T: serde::de::DeserializeOwned>(cell: Option<&String>, what: &str) -> Result<T> {
    let text = cell.map(String::as_str).unwrap_or_default();
    serde_json::from_str(text)
        .map_err(|e| StoreError::Corrupt(format!("{what} document does not parse: {e}")))
}

fn finite(value: f64) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(invalid("result", "values must be finite"))
    }
}

fn invalid(what: &'static str, reason: &str) -> StoreError {
    StoreError::Invalid {
        what,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_core::ProjectId;
    use research_core::ProjectManifest;

    fn open_temp() -> (tempfile::TempDir, ProjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "Study", 0).unwrap();
        let project = ProjectStore::create(&dir.path().join("p"), &manifest).unwrap();
        (dir, project)
    }

    fn run(id: &str, started: u64) -> Run {
        Run {
            id: RunId::new(id).unwrap(),
            label: format!("run {id}"),
            inputs: BTreeMap::from([("k".to_string(), "2.5".to_string())]),
            code_hash: "abc123".into(),
            environment: BTreeMap::from([("seed".to_string(), "7".to_string())]),
            started_unix: started,
            finished_unix: started + 10,
        }
    }

    fn result(id: &str, run_id: &str, name: &str, value: Value) -> StoredResult {
        StoredResult {
            id: ResultId::new(id).unwrap(),
            run_id: RunId::new(run_id).unwrap(),
            name: name.into(),
            value,
        }
    }

    #[test]
    fn runs_are_saved_replaced_listed_and_loaded() {
        let (_dir, project) = open_temp();
        project.save_run(&run("b", 200)).unwrap();
        project.save_run(&run("a", 100)).unwrap();

        let mut changed = run("b", 200);
        changed.label = "renamed".into();
        project.save_run(&changed).unwrap();

        let ids: Vec<String> = project
            .list_runs()
            .unwrap()
            .iter()
            .map(|r| r.id.to_string())
            .collect();
        assert_eq!(ids, ["a", "b"]);
        let loaded = project
            .load_run(&RunId::new("b").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(loaded.label, "renamed");
        assert!(project
            .load_run(&RunId::new("zz").unwrap())
            .unwrap()
            .is_none());
    }

    #[test]
    fn runs_can_be_filtered_by_input() {
        let (_dir, project) = open_temp();
        project.save_run(&run("a", 1)).unwrap();
        let mut other = run("b", 2);
        other.inputs.insert("k".into(), "9".into());
        project.save_run(&other).unwrap();

        let hits = project.runs_with_input("k", "2.5").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_str(), "a");
    }

    #[test]
    fn run_validation_rejects_bad_fields() {
        let (_dir, project) = open_temp();
        let mut r = run("a", 100);
        r.finished_unix = 50;
        assert!(matches!(
            project.save_run(&r),
            Err(StoreError::Invalid { .. })
        ));

        let mut blank = run("a", 100);
        blank.label = "  ".into();
        assert!(project.save_run(&blank).is_err());
    }

    #[test]
    fn every_value_kind_round_trips_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("p");
        let manifest = ProjectManifest::new(ProjectId::new("p").unwrap(), "S", 0).unwrap();
        let values = [
            Value::Scalar { value: 1.5 },
            Value::Array {
                values: vec![0.0, -2.0, 3.25],
            },
            Value::Table {
                columns: vec!["t".into(), "x".into()],
                rows: vec![vec![0.0, 1.0], vec![0.1, 0.9]],
            },
            Value::Text {
                text: "converged in 12 steps".into(),
            },
        ];
        {
            let project = ProjectStore::create(&root, &manifest).unwrap();
            project.save_run(&run("r1", 5)).unwrap();
            for (i, v) in values.iter().enumerate() {
                project
                    .save_result(&result(
                        &format!("res{i}"),
                        "r1",
                        &format!("out{i}"),
                        v.clone(),
                    ))
                    .unwrap();
            }
        }
        let project = ProjectStore::open(&root).unwrap();
        let stored = project.results_for_run(&RunId::new("r1").unwrap()).unwrap();
        assert_eq!(stored.len(), values.len());
        for (i, v) in values.iter().enumerate() {
            let found = stored.iter().find(|r| r.name == format!("out{i}")).unwrap();
            assert_eq!(&found.value, v);
        }
    }

    #[test]
    fn result_requires_an_existing_run() {
        let (_dir, project) = open_temp();
        let r = result("x", "missing", "n", Value::Scalar { value: 1.0 });
        assert!(matches!(
            project.save_result(&r),
            Err(StoreError::RunNotFound(_))
        ));
    }

    #[test]
    fn non_finite_and_malformed_values_are_rejected() {
        let (_dir, project) = open_temp();
        project.save_run(&run("r", 1)).unwrap();
        let nan = result("n", "r", "nan", Value::Scalar { value: f64::NAN });
        assert!(project.save_result(&nan).is_err());
        let inf = result(
            "i",
            "r",
            "inf",
            Value::Array {
                values: vec![f64::INFINITY],
            },
        );
        assert!(project.save_result(&inf).is_err());
        let ragged = result(
            "t",
            "r",
            "tab",
            Value::Table {
                columns: vec!["a".into(), "b".into()],
                rows: vec![vec![1.0]],
            },
        );
        assert!(project.save_result(&ragged).is_err());
    }

    #[test]
    fn deleting_a_run_removes_its_results_only() {
        let (_dir, project) = open_temp();
        project.save_run(&run("a", 1)).unwrap();
        project.save_run(&run("b", 2)).unwrap();
        project
            .save_result(&result("ra", "a", "n", Value::Scalar { value: 1.0 }))
            .unwrap();
        project
            .save_result(&result("rb", "b", "n", Value::Scalar { value: 2.0 }))
            .unwrap();

        project.delete_run(&RunId::new("a").unwrap()).unwrap();
        assert!(project
            .load_run(&RunId::new("a").unwrap())
            .unwrap()
            .is_none());
        assert!(project
            .results_for_run(&RunId::new("a").unwrap())
            .unwrap()
            .is_empty());
        assert_eq!(
            project
                .results_for_run(&RunId::new("b").unwrap())
                .unwrap()
                .len(),
            1
        );
        // Deleting again is a no-op.
        project.delete_run(&RunId::new("a").unwrap()).unwrap();
    }

    #[test]
    fn names_with_quotes_are_stored_safely() {
        let (_dir, project) = open_temp();
        let mut r = run("q", 1);
        r.label = "O'Brien's run".into();
        project.save_run(&r).unwrap();
        let loaded = project
            .load_run(&RunId::new("q").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(loaded.label, "O'Brien's run");
    }

    #[test]
    fn plot_result_refers_to_a_stored_blob() {
        let (_dir, project) = open_temp();
        project.save_run(&run("r", 1)).unwrap();
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let hash = project.put_blob(svg).unwrap();
        let plot = result(
            "plot",
            "r",
            "trajectory",
            Value::Blob {
                media_type: "image/svg+xml".into(),
                sha256: hash.clone(),
            },
        );
        project.save_result(&plot).unwrap();

        let stored = project.results_for_run(&plot.run_id).unwrap();
        assert_eq!(stored[0].value, plot.value);
        assert_eq!(project.get_blob(&hash).unwrap().unwrap(), svg);
    }

    #[test]
    fn blob_result_without_its_blob_is_refused() {
        let (_dir, project) = open_temp();
        project.save_run(&run("r", 1)).unwrap();
        let missing = "a".repeat(64);
        let r = result(
            "p",
            "r",
            "image",
            Value::Blob {
                media_type: "image/png".into(),
                sha256: missing.clone(),
            },
        );
        assert!(matches!(
            project.save_result(&r),
            Err(StoreError::BlobNotFound(h)) if h == missing
        ));
    }

    #[test]
    fn blob_value_metadata_is_validated() {
        let (_dir, project) = open_temp();
        project.save_run(&run("r", 1)).unwrap();
        let hash = project.put_blob(b"x").unwrap();
        for media_type in ["", "  ", "image/png\n"] {
            let r = result(
                "p",
                "r",
                "img",
                Value::Blob {
                    media_type: media_type.into(),
                    sha256: hash.clone(),
                },
            );
            assert!(project.save_result(&r).is_err(), "accepted {media_type:?}");
        }
        let bad_hash = result(
            "p",
            "r",
            "img",
            Value::Blob {
                media_type: "image/png".into(),
                sha256: "not-a-hash".into(),
            },
        );
        assert!(project.save_result(&bad_hash).is_err());
    }

    #[test]
    fn comparison_shows_differing_inputs_and_per_run_results() {
        let (_dir, project) = open_temp();
        let mut a = run("a", 1);
        a.inputs.insert("k".into(), "2.5".into());
        let mut b = run("b", 2);
        b.inputs.insert("k".into(), "9".into());
        // Both runs share "seed"; "mode" is only on "b", so it differs too.
        a.inputs.insert("seed".into(), "7".into());
        b.inputs.insert("seed".into(), "7".into());
        b.inputs.insert("mode".into(), "fast".into());
        project.save_run(&a).unwrap();
        project.save_run(&b).unwrap();
        project
            .save_result(&result("ra", "a", "energy", Value::Scalar { value: 1.0 }))
            .unwrap();
        project
            .save_result(&result("rb", "b", "energy", Value::Scalar { value: 2.0 }))
            .unwrap();
        project
            .save_result(&result(
                "rb2",
                "b",
                "trace",
                Value::Text { text: "ok".into() },
            ))
            .unwrap();

        let ids = [RunId::new("a").unwrap(), RunId::new("b").unwrap()];
        let cmp = project.compare_runs(&ids).unwrap();
        assert_eq!(cmp.runs.len(), 2);
        assert_eq!(cmp.differing_inputs, ["k", "mode"]);
        assert_eq!(
            cmp.results,
            vec![
                (
                    "energy".to_string(),
                    vec![
                        Some(Value::Scalar { value: 1.0 }),
                        Some(Value::Scalar { value: 2.0 })
                    ]
                ),
                (
                    "trace".to_string(),
                    vec![None, Some(Value::Text { text: "ok".into() })]
                ),
            ]
        );
    }

    #[test]
    fn comparing_an_unknown_run_is_an_error() {
        let (_dir, project) = open_temp();
        project.save_run(&run("a", 1)).unwrap();
        let ids = [RunId::new("a").unwrap(), RunId::new("nope").unwrap()];
        assert!(matches!(
            project.compare_runs(&ids),
            Err(StoreError::RunNotFound(id)) if id == "nope"
        ));
    }
}
