//! A named, time-indexed trajectory with CSV output.
//!
//! [`Trajectory`] is the uniform output shape for dense ODE solutions: one row
//! per output time, one named column per state variable. It is plain data, so
//! it can be inspected, plotted, or written out with [`Trajectory::to_csv`].

use std::fmt::Write as _;
use std::io;

use crate::error::OdeError;
use crate::problem::OdeProblem;
use crate::solver::Method;

/// Time-indexed states with a name per state variable.
///
/// Invariants (enforced by [`Trajectory::new`]): `t.len() == y.len()`, and every
/// row of `y` has `names.len()` entries.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Trajectory {
    /// Output times, in the order they were produced.
    pub t: Vec<f64>,
    /// Column names, one per state variable.
    pub names: Vec<String>,
    /// `y[k]` is the state at `t[k]`.
    pub y: Vec<Vec<f64>>,
}

impl Trajectory {
    /// Build a trajectory, validating that the shapes agree.
    ///
    /// # Errors
    ///
    /// Returns [`OdeError::Invalid`] if `t` and `y` differ in length, or if any
    /// state row does not have one entry per name.
    pub fn new(t: Vec<f64>, names: Vec<String>, y: Vec<Vec<f64>>) -> Result<Self, OdeError> {
        if t.len() != y.len() {
            return Err(OdeError::invalid(format!(
                "trajectory has {} times but {} states",
                t.len(),
                y.len()
            )));
        }
        if let Some((k, row)) = y.iter().enumerate().find(|(_, r)| r.len() != names.len()) {
            return Err(OdeError::invalid(format!(
                "state row {k} has {} entries, expected {}",
                row.len(),
                names.len()
            )));
        }
        Ok(Self { t, names, y })
    }

    /// Number of output rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.t.len()
    }

    /// Whether the trajectory has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    /// The values of one named state variable across all rows, or `None` if no
    /// column has that name.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<Vec<f64>> {
        let idx = self.names.iter().position(|n| n == name)?;
        Some(self.y.iter().map(|row| row[idx]).collect())
    }

    /// Render as CSV: a `t` column followed by one column per state variable,
    /// one row per output time, with a header line. Numbers use Rust's
    /// shortest round-trip `Display` form.
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push('t');
        for name in &self.names {
            out.push(',');
            out.push_str(&escape_csv_field(name));
        }
        out.push('\n');
        for (t, row) in self.t.iter().zip(&self.y) {
            // Writing to a `String` cannot fail.
            let _ = write!(out, "{t}");
            for v in row {
                let _ = write!(out, ",{v}");
            }
            out.push('\n');
        }
        out
    }

    /// Write the CSV form (see [`Trajectory::to_csv`]) to any writer.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from `writer`.
    pub fn write_csv<W: io::Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(self.to_csv().as_bytes())
    }
}

impl OdeProblem {
    /// Integrate and return the dense output as a named [`Trajectory`].
    ///
    /// `names` must have one entry per state variable. Output is produced at each
    /// point of `t_eval` (see [`OdeProblem::solve_dense`]).
    ///
    /// # Errors
    ///
    /// Returns [`OdeError::Invalid`] if `names` does not match the state size,
    /// or any error from the integration itself.
    pub fn solve_trajectory(
        &self,
        method: Method,
        t_eval: &[f64],
        names: &[&str],
    ) -> Result<Trajectory, OdeError> {
        if names.len() != self.nstates() {
            return Err(OdeError::invalid(format!(
                "{} names given for {} state variables",
                names.len(),
                self.nstates()
            )));
        }
        let y = self.solve_dense(method, t_eval)?;
        let names = names.iter().map(|n| (*n).to_string()).collect();
        Trajectory::new(t_eval.to_vec(), names, y)
    }
}

/// Quote a CSV field if it contains a delimiter, quote, or newline.
fn escape_csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Trajectory {
        Trajectory::new(
            vec![0.0, 0.5],
            vec!["x".into(), "a,b".into()],
            vec![vec![1.0, 2.0], vec![3.0, 4.5]],
        )
        .unwrap()
    }

    #[test]
    fn csv_has_header_and_one_row_per_time() {
        let csv = sample().to_csv();
        assert_eq!(csv, "t,x,\"a,b\"\n0,1,2\n0.5,3,4.5\n");
    }

    #[test]
    fn column_lookup_by_name() {
        let tr = sample();
        assert_eq!(tr.column("x"), Some(vec![1.0, 3.0]));
        assert_eq!(tr.column("missing"), None);
    }

    #[test]
    fn rejects_mismatched_shapes() {
        let bad_len = Trajectory::new(vec![0.0], vec!["x".into()], vec![]);
        assert!(bad_len.is_err());
        let bad_row = Trajectory::new(vec![0.0], vec!["x".into()], vec![vec![1.0, 2.0]]);
        assert!(bad_row.is_err());
    }

    #[test]
    fn write_csv_matches_to_csv() {
        let tr = sample();
        let mut buf = Vec::new();
        tr.write_csv(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), tr.to_csv());
    }
}
