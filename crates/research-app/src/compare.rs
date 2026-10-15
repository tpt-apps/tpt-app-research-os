//! Text for the run comparison window. `compare_lines` turns a [`RunComparison`] into
//! lines of text, one column per run, so the comparison can be read without a table
//! widget. Nothing here draws anything.

use research_store::{RunComparison, Value};

/// How many array entries to show before summarising the rest.
const SHOWN_VALUES: usize = 8;

/// A short description of one result value.
pub fn describe_value(value: &Value) -> String {
    match value {
        Value::Scalar { value } => format!("{value}"),
        Value::Array { values } => {
            let shown: Vec<String> = values
                .iter()
                .take(SHOWN_VALUES)
                .map(|v| format!("{v}"))
                .collect();
            if values.len() > SHOWN_VALUES {
                format!("[{}, … {} values]", shown.join(", "), values.len())
            } else {
                format!("[{}]", shown.join(", "))
            }
        }
        Value::Table { columns, rows } => {
            format!("table {} × {}", rows.len(), columns.len())
        }
        Value::Text { text } => text.clone(),
        Value::Blob { media_type, sha256 } => {
            let short: String = sha256.chars().take(12).collect();
            format!("{media_type} ({short}…)")
        }
    }
}

/// The comparison as text: which runs were compared, the inputs that differ, then
/// each result with one entry per run (`—` where a run has no result of that name).
pub fn compare_lines(cmp: &RunComparison) -> Vec<String> {
    let mut lines = vec![format!("Comparing {} runs:", cmp.runs.len())];
    for (i, run) in cmp.runs.iter().enumerate() {
        lines.push(format!("  {}. {} ({})", i + 1, run.label, run.id));
    }

    if cmp.differing_inputs.is_empty() {
        lines.push("Inputs: all runs share the same inputs.".into());
    } else {
        lines.push("Inputs that differ:".into());
        for key in &cmp.differing_inputs {
            let values: Vec<String> = cmp
                .runs
                .iter()
                .map(|r| r.inputs.get(key).cloned().unwrap_or_else(|| "—".into()))
                .collect();
            lines.push(format!("  {key}: {}", values.join(" | ")));
        }
    }

    if cmp.results.is_empty() {
        lines.push("Results: none recorded.".into());
    } else {
        lines.push("Results:".into());
        for (name, values) in &cmp.results {
            let cells: Vec<String> = values
                .iter()
                .map(|v| v.as_ref().map_or_else(|| "—".into(), describe_value))
                .collect();
            lines.push(format!("  {name}: {}", cells.join(" | ")));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use research_store::Run;
    use std::collections::BTreeMap;

    fn run(id: &str, k: &str) -> Run {
        let mut inputs = BTreeMap::new();
        inputs.insert("k".to_string(), k.to_string());
        Run {
            id: research_core::RunId::new(id).unwrap(),
            label: format!("run {id}"),
            inputs,
            code_hash: "h".into(),
            environment: BTreeMap::new(),
            started_unix: 1,
            finished_unix: 2,
        }
    }

    #[test]
    fn values_are_described_briefly() {
        assert_eq!(describe_value(&Value::Scalar { value: 2.5 }), "2.5");
        assert_eq!(
            describe_value(&Value::Array {
                values: vec![1.0, 2.0]
            }),
            "[1, 2]"
        );
        let long = Value::Array {
            values: (0..10).map(f64::from).collect(),
        };
        assert!(describe_value(&long).ends_with("… 10 values]"));
        assert_eq!(
            describe_value(&Value::Table {
                columns: vec!["t".into(), "y".into()],
                rows: vec![vec![0.0, 1.0]],
            }),
            "table 1 × 2"
        );
    }

    #[test]
    fn the_comparison_lists_differing_inputs_and_missing_results() {
        let cmp = RunComparison {
            runs: vec![run("r1", "2.0"), run("r2", "3.0")],
            differing_inputs: vec!["k".into()],
            results: vec![(
                "energy".into(),
                vec![Some(Value::Scalar { value: 1.0 }), None],
            )],
        };
        let text = compare_lines(&cmp).join("\n");
        assert!(text.contains("Comparing 2 runs:"));
        assert!(text.contains("k: 2.0 | 3.0"));
        assert!(text.contains("energy: 1 | —"));
    }

    #[test]
    fn identical_inputs_are_said_so() {
        let cmp = RunComparison {
            runs: vec![run("r1", "2.0")],
            differing_inputs: Vec::new(),
            results: Vec::new(),
        };
        let text = compare_lines(&cmp).join("\n");
        assert!(text.contains("all runs share the same inputs"));
        assert!(text.contains("Results: none recorded."));
    }
}
