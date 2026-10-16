//! Running a document's code cells in order, with a shared Rhai scope.
//!
//! A cell is re-run only if its source changed since its last run, or if an
//! earlier cell re-ran, because its variables may have changed. Other cells keep
//! their stored outputs.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use research_core::{CellId, ResultId, RunId};
use research_doc::{CellBody, Document, Output};
use research_store::{ProjectStore, Run, StoredResult, Value};
use rhai::{Dynamic, EvalAltResult, Scope};
use sha2::{Digest, Sha256};

use crate::engine::{self, Control, Limits, Sinks, LANGUAGE};
use crate::error::Result;

/// What one call to [`Session::run_document`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunSummary {
    /// Cells that ran, in order.
    pub ran: Vec<CellId>,
    /// Cells whose stored outputs were kept.
    pub reused: Vec<CellId>,
    /// True if the run was cancelled before every cell was considered.
    pub cancelled: bool,
}

/// Where a cell's run is recorded, and the identifiers it needs.
pub struct Recording<'a> {
    pub store: &'a ProjectStore,
    /// Mixed into generated run ids, so runs from different documents never collide.
    pub run_prefix: &'a str,
}

/// Variables and cached state for one document session.
///
/// The scope persists across runs, so later cells see earlier cells' variables.
/// A new `Session` starts with an empty scope.
pub struct Session {
    seed: u64,
    limits: Limits,
    scope: Scope<'static>,
    fingerprints: BTreeMap<CellId, String>,
    runs_recorded: u64,
}

struct CellOutcome {
    outputs: Vec<Output>,
    value: Option<f64>,
    cancelled: bool,
}

impl Session {
    /// A session with the given deterministic seed and limits.
    pub fn new(seed: u64, limits: Limits) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            seed,
            limits,
            scope: Scope::new(),
            fingerprints: BTreeMap::new(),
            runs_recorded: 0,
        })
    }

    /// Run the code cells that need it, updating their outputs in place.
    ///
    /// With `recording`, each cell that runs is saved as a run, and its outputs and
    /// value are stored as results. A failing cell does not stop the run; its error
    /// goes into its outputs.
    pub fn run_document(
        &mut self,
        doc: &mut Document,
        control: &Control,
        recording: Option<Recording<'_>>,
    ) -> Result<RunSummary> {
        let mut summary = RunSummary::default();
        let mut dirty = false;

        for cell in &mut doc.cells {
            if control.is_cancelled() {
                summary.cancelled = true;
                break;
            }
            let CellBody::Code {
                language,
                source,
                outputs,
            } = &mut cell.body
            else {
                continue;
            };

            if language != LANGUAGE {
                *outputs = vec![Output::Error {
                    name: "UnsupportedLanguage".into(),
                    message: format!("{language} cells cannot run in this build"),
                }];
                dirty = true;
                continue;
            }

            let fingerprint = fingerprint(source);
            if !dirty && self.fingerprints.get(&cell.id) == Some(&fingerprint) {
                summary.reused.push(cell.id.clone());
                continue;
            }

            let started = unix_now();
            let outcome = self.run_source(source, control);
            let finished = unix_now();
            *outputs = outcome.outputs.clone();
            dirty = true;

            if outcome.cancelled {
                // Not cached, so the next run executes this cell again.
                self.fingerprints.remove(&cell.id);
                summary.cancelled = true;
                break;
            }
            self.fingerprints.insert(cell.id.clone(), fingerprint);
            summary.ran.push(cell.id.clone());

            if let Some(recording) = &recording {
                self.record(recording, &cell.id, source, started, finished, &outcome)?;
            }
        }
        Ok(summary)
    }

    fn run_source(&mut self, source: &str, control: &Control) -> CellOutcome {
        let sinks = Sinks::default();
        let deadline = Instant::now() + self.limits.timeout;
        let engine = engine::build(&self.limits, control, deadline, self.seed, sinks.clone());

        let result = engine.eval_with_scope::<Dynamic>(&mut self.scope, source);
        let mut outputs = Vec::new();
        let printed = sinks.prints.borrow().concat();
        if !printed.is_empty() {
            outputs.push(Output::Text { text: printed });
        }
        outputs.extend(sinks.shown.borrow().iter().cloned());

        match result {
            Ok(value) => {
                let mut numeric = None;
                if !value.is_unit() {
                    if let Ok(f) = value.as_float() {
                        numeric = Some(f);
                    } else if let Ok(i) = value.as_int() {
                        numeric = Some(i as f64);
                    }
                    outputs.push(Output::Text {
                        text: value.to_string(),
                    });
                }
                CellOutcome {
                    outputs,
                    value: numeric,
                    cancelled: false,
                }
            }
            Err(err) => {
                let (name, cancelled) = classify(&err, control);
                outputs.push(Output::Error {
                    name: name.into(),
                    message: err.to_string(),
                });
                CellOutcome {
                    outputs,
                    value: None,
                    cancelled,
                }
            }
        }
    }

    fn record(
        &mut self,
        recording: &Recording<'_>,
        cell: &CellId,
        source: &str,
        started: u64,
        finished: u64,
        outcome: &CellOutcome,
    ) -> Result<()> {
        self.runs_recorded += 1;
        let run_id = RunId::new(format!(
            "run-{}",
            short_hash(&[
                recording.run_prefix,
                cell.as_str(),
                &started.to_string(),
                &self.runs_recorded.to_string(),
            ])
        ))?;
        recording.store.save_run(&Run {
            id: run_id.clone(),
            label: format!("cell {cell}"),
            inputs: BTreeMap::from([
                ("language".to_string(), LANGUAGE.to_string()),
                ("seed".to_string(), self.seed.to_string()),
            ]),
            code_hash: fingerprint(source),
            environment: BTreeMap::from([
                ("compute".to_string(), env!("CARGO_PKG_VERSION").to_string()),
                (
                    "timeout_ms".to_string(),
                    self.limits.timeout.as_millis().to_string(),
                ),
                (
                    "max_operations".to_string(),
                    self.limits.max_operations.to_string(),
                ),
            ]),
            started_unix: started,
            finished_unix: finished.max(started),
        })?;

        let mut results = Vec::new();
        if let Some(value) = outcome.value {
            results.push(("value".to_string(), Value::Scalar { value }));
        }
        for (i, output) in outcome.outputs.iter().enumerate() {
            let named = match output {
                Output::Text { text } => {
                    (format!("output-{i}"), Value::Text { text: text.clone() })
                }
                Output::Error { name, message } => (
                    format!("error-{i}"),
                    Value::Text {
                        text: format!("{name}: {message}"),
                    },
                ),
                Output::Svg { svg } => {
                    let sha256 = recording.store.put_blob(svg.as_bytes())?;
                    (
                        format!("plot-{i}"),
                        Value::Blob {
                            media_type: "image/svg+xml".into(),
                            sha256,
                        },
                    )
                }
                Output::Table { .. } => continue,
            };
            results.push(named);
        }
        for (name, value) in results {
            let id = ResultId::new(format!("res-{}", short_hash(&[run_id.as_str(), &name])))?;
            recording.store.save_result(&StoredResult {
                id,
                run_id: run_id.clone(),
                name,
                value,
            })?;
        }
        Ok(())
    }
}

/// Map an evaluation error to a stable name, and whether it was a cancellation.
fn classify(err: &EvalAltResult, control: &Control) -> (&'static str, bool) {
    match err {
        EvalAltResult::ErrorTerminated(..) if control.is_cancelled() => ("Cancelled", true),
        EvalAltResult::ErrorTerminated(..) => ("Timeout", false),
        EvalAltResult::ErrorTooManyOperations(..) => ("OperationLimit", false),
        EvalAltResult::ErrorDataTooLarge(..) => ("MemoryLimit", false),
        EvalAltResult::ErrorStackOverflow(..) => ("CallDepthLimit", false),
        _ => ("RuntimeError", false),
    }
}

/// Identifies a cell's source. A changed source gives a different fingerprint.
fn fingerprint(source: &str) -> String {
    short_hash(&[LANGUAGE, source])
}

/// The first 16 bytes of SHA-256 over the parts, as hex. Parts are separated by NUL.
fn short_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
