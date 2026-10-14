//! In-crate sparse linear algebra for the implicit ODE solvers.
//!
//! A compressed-sparse-row ([`CsrMatrix`]) type plus a sparse LU decomposition
//! with partial pivoting ([`CsrMatrix::sparse_solve`]). This complements the
//! dense `DMat` (in the `linalg` module): for systems with at least 64 states
//! (the `SPARSE_LU_MIN_N` threshold), the implicit SDIRK and BDF solvers build
//! their Newton matrices in CSR storage and factor them with the sparse LU
//! below, so the dense `n × n` matrix is never formed.
//!
//! The factorisation is left-looking (Gilbert–Peierls): each column is solved
//! against the already-computed `L` restricted to its reachable pattern, so the
//! work and fill follow the sparsity of the matrix rather than `n²`. Pivoting is
//! partial (largest magnitude in the column among the not-yet-pivoted rows).
//! No fill-reducing column ordering is applied, so heavy fill is possible for
//! matrices with unfavourable structure.

use crate::error::OdeError;
use crate::linalg::{NEWTON_TOL, Tol};

/// Minimum system size for the implicit solvers to route their Newton linear
/// solves through the sparse CSR/LU path instead of the dense `DMat` path.
/// Below this threshold the dense factorisation is faster (no CSR build
/// overhead).
pub(crate) const SPARSE_LU_MIN_N: usize = 64;

/// Sentinel for "no value" in index arrays.
const NONE: usize = usize::MAX;

/// Compressed-sparse-row matrix.
///
/// Stores only the non-zero entries `data` together with their column indices
/// `indices` (per row, in ascending column order) and the CSR row-pointer
/// `indptr` (length `nrows + 1`). A zero entry must not be stored.
#[derive(Clone)]
pub struct CsrMatrix {
    nrows: usize,
    ncols: usize,
    /// Row pointers: `indptr[r] .. indptr[r+1]` indexes the slice of row `r`'s
    /// non-zeros in `data` / `indices`.
    indptr: Vec<usize>,
    /// Column index of each stored entry (ascending within a row).
    indices: Vec<usize>,
    /// Value of each stored entry.
    data: Vec<f64>,
}

impl CsrMatrix {
    /// Number of rows.
    #[must_use]
    pub fn nrows(&self) -> usize {
        self.nrows
    }

    /// Number of columns.
    #[must_use]
    pub fn ncols(&self) -> usize {
        self.ncols
    }

    /// Build a CSR matrix from a dense row-major slice (row length `ncols`).
    /// Zero entries are dropped.
    #[must_use]
    pub fn from_dense(dense: &[f64], nrows: usize, ncols: usize) -> Self {
        debug_assert_eq!(dense.len(), nrows * ncols);
        let mut indptr = vec![0usize; nrows + 1];
        let mut indices = Vec::new();
        let mut data = Vec::new();
        for r in 0..nrows {
            let row = &dense[r * ncols..(r + 1) * ncols];
            for (c, &v) in row.iter().enumerate() {
                if v != 0.0 {
                    indices.push(c);
                    data.push(v);
                }
            }
            indptr[r + 1] = indices.len();
        }
        CsrMatrix {
            nrows,
            ncols,
            indptr,
            indices,
            data,
        }
    }

    /// Number of explicitly stored non-zeros.
    #[must_use]
    pub fn nnz(&self) -> usize {
        self.data.len()
    }

    /// Look up the stored value at `(r, c)`, or `0.0` if not present.
    #[must_use]
    pub fn get(&self, r: usize, c: usize) -> f64 {
        let start = self.indptr[r];
        let end = self.indptr[r + 1];
        match self.indices[start..end].binary_search(&c) {
            Ok(pos) => self.data[start + pos],
            Err(_) => 0.0,
        }
    }

    /// `y = A x`.
    ///
    /// # Panics
    ///
    /// Panics if `x.len() != self.ncols()`.
    #[must_use]
    pub fn mat_vec(&self, x: &[f64]) -> Vec<f64> {
        assert_eq!(x.len(), self.ncols, "vector length must match ncols");
        let mut y = vec![0.0; self.nrows];
        for (r, yr) in y.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in self.indptr[r]..self.indptr[r + 1] {
                s += self.data[k] * x[self.indices[k]];
            }
            *yr = s;
        }
        y
    }

    /// Build the Newton matrix `A = I − diag·self` directly in CSR storage,
    /// merging the unit diagonal with the scaled Jacobian entries per row
    /// (both in ascending column order) without ever densifying.
    #[must_use]
    pub fn scaled_identity_minus_scaled(&self, diag: f64) -> CsrMatrix {
        debug_assert_eq!(self.ncols, self.nrows);
        let n = self.nrows;
        let mut indptr = vec![0usize; n + 1];
        let mut indices = Vec::new();
        let mut data = Vec::new();
        for r in 0..n {
            let start = self.indptr[r];
            let end = self.indptr[r + 1];
            let mut k = start;
            // Merge the unit diagonal with the scaled Jacobian row (both in
            // ascending column order).
            while k < end && self.indices[k] < r {
                indices.push(self.indices[k]);
                data.push(-diag * self.data[k]);
                k += 1;
            }
            if k < end && self.indices[k] == r {
                indices.push(r);
                data.push(1.0 - diag * self.data[k]);
                k += 1;
            } else {
                indices.push(r);
                data.push(1.0);
            }
            while k < end {
                indices.push(self.indices[k]);
                data.push(-diag * self.data[k]);
                k += 1;
            }
            indptr[r + 1] = indices.len();
        }
        CsrMatrix {
            nrows: n,
            ncols: n,
            indptr,
            indices,
            data,
        }
    }

    /// Convert to a dense row-major `Vec<f64>` (for testing / verification).
    #[cfg(test)]
    pub fn to_dense(&self) -> Vec<f64> {
        let mut d = vec![0.0; self.nrows * self.ncols];
        for r in 0..self.nrows {
            for k in self.indptr[r]..self.indptr[r + 1] {
                d[r * self.ncols + self.indices[k]] = self.data[k];
            }
        }
        d
    }

    /// Build the full Jacobian `J = df/dy` at `(t, y)` via forward finite
    /// differences, returned as a `CsrMatrix`. `f0` must be `f(t, y)` and `cols`
    /// is the set of columns to perturb (a sparsity pattern, or `None` for the
    /// full dense pattern). When `cols` is `None` every column is finite-
    /// differenced, matching the dense `linalg::jacobian` helper but in
    /// compressed storage (exact-zero entries are dropped).
    ///
    /// # Errors
    ///
    /// Propagates any error returned by the RHS `f`.
    pub fn jacobian<F>(
        f: F,
        t: f64,
        y: &[f64],
        f0: &[f64],
        cols: Option<&[usize]>,
    ) -> Result<CsrMatrix, OdeError>
    where
        F: Fn(f64, &[f64], &mut [f64]) -> Result<(), OdeError>,
    {
        let n = y.len();
        let sqrt_eps = f64::sqrt(f64::EPSILON);
        let perturb: Vec<usize> = match cols {
            Some(c) => c.to_vec(),
            None => (0..n).collect(),
        };
        // Collect COO triplets (row, col, val). Perturbing column `j` yields the
        // whole of Jacobian column `j`: `val = (f_row(y+dy·e_j) - f_row(y))/dy`.
        let mut rows = Vec::new();
        let mut cols = Vec::new();
        let mut vals = Vec::new();
        for &col in &perturb {
            let dy = sqrt_eps * y[col].abs().max(1.0);
            let mut yp = y.to_vec();
            yp[col] += dy;
            let mut fp = vec![0.0; n];
            f(t, &yp, &mut fp)?;
            let inv = 1.0 / dy;
            for row in 0..n {
                let val = (fp[row] - f0[row]) * inv;
                if val != 0.0 {
                    rows.push(row);
                    cols.push(col);
                    vals.push(val);
                }
            }
        }
        // Sort triplets by (row, col) so each row's entries are in ascending
        // column order, then compress into CSR.
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| (rows[a], cols[a]).cmp(&(rows[b], cols[b])));
        let mut indptr = vec![0usize; n + 1];
        for &k in &order {
            indptr[rows[k] + 1] += 1;
        }
        for r in 0..n {
            indptr[r + 1] += indptr[r];
        }
        let mut indices = Vec::with_capacity(vals.len());
        let mut data = Vec::with_capacity(vals.len());
        for &k in &order {
            indices.push(cols[k]);
            data.push(vals[k]);
        }
        Ok(CsrMatrix {
            nrows: n,
            ncols: n,
            indptr,
            indices,
            data,
        })
    }

    /// Solve `A x = b` via sparse LU with partial pivoting.
    ///
    /// Returns `None` if the matrix is (numerically) singular or the dimensions
    /// mismatch.
    #[must_use]
    pub fn sparse_solve(&self, b: &[f64]) -> Option<Vec<f64>> {
        let n = self.nrows;
        if self.ncols != n || b.len() != n {
            return None;
        }
        let lu = SparseLu::factor(self)?;
        Some(lu.solve(b))
    }
}

/// Sparse LU factors of `P·A = L·U`, stored by column.
///
/// Pivot step `k` eliminates original row `prow[k]`. Column `k` of `L` holds the
/// multipliers for the rows not yet pivoted at that step, and column `k` of `U`
/// holds the entries above the diagonal, indexed by pivot step.
struct SparseLu {
    n: usize,
    /// `prow[k]`: original row chosen as the pivot at step `k`.
    prow: Vec<usize>,
    /// Column `k` of `L` below the diagonal: `(original row, multiplier)`.
    lcols: Vec<Vec<(usize, f64)>>,
    /// Column `k` of `U` above the diagonal: `(step i < k, value)`.
    ucols: Vec<Vec<(usize, f64)>>,
    /// Diagonal of `U`.
    udiag: Vec<f64>,
}

impl SparseLu {
    /// Left-looking (Gilbert–Peierls) factorisation with partial pivoting.
    /// Returns `None` if a column has no usable pivot.
    fn factor(a: &CsrMatrix) -> Option<SparseLu> {
        let n = a.nrows;
        let nnz = a.data.len();

        // Column-compressed copy of A, so each column can be scattered directly.
        let mut col_ptr = vec![0usize; n + 1];
        for &c in &a.indices {
            col_ptr[c + 1] += 1;
        }
        for c in 0..n {
            col_ptr[c + 1] += col_ptr[c];
        }
        let mut fill = col_ptr.clone();
        let mut rows_csc = vec![0usize; nnz];
        let mut vals_csc = vec![0.0; nnz];
        for r in 0..n {
            for k in a.indptr[r]..a.indptr[r + 1] {
                let c = a.indices[k];
                rows_csc[fill[c]] = r;
                vals_csc[fill[c]] = a.data[k];
                fill[c] += 1;
            }
        }

        let scale = a.data.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
        let mut pinv = vec![NONE; n]; // original row -> pivot step
        let mut prow = vec![NONE; n]; // pivot step -> original row
        let mut lcols: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut ucols: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut udiag = vec![0.0; n];
        let mut x = vec![0.0; n]; // dense work vector, indexed by original row
        let mut mark = vec![NONE; n]; // DFS visit stamp, per column
        let mut pattern: Vec<usize> = Vec::new(); // reached rows, in postorder
        let mut stack: Vec<(usize, usize)> = Vec::new();

        for k in 0..n {
            pattern.clear();
            for p in col_ptr[k]..col_ptr[k + 1] {
                x[rows_csc[p]] = vals_csc[p];
            }

            // Reach of the column's nonzero rows through the pivoted part of L.
            for &r0 in &rows_csc[col_ptr[k]..col_ptr[k + 1]] {
                if mark[r0] == k {
                    continue;
                }
                mark[r0] = k;
                stack.push((r0, 0));
                while let Some(top) = stack.last_mut() {
                    let r = top.0;
                    let children: &[(usize, f64)] = if pinv[r] == NONE {
                        &[]
                    } else {
                        &lcols[pinv[r]]
                    };
                    if top.1 < children.len() {
                        let child = children[top.1].0;
                        top.1 += 1;
                        if mark[child] != k {
                            mark[child] = k;
                            stack.push((child, 0));
                        }
                    } else {
                        pattern.push(r);
                        stack.pop();
                    }
                }
            }

            // Sparse triangular solve: apply the earlier pivots in topological
            // order (reverse postorder), so each row is final before it is used.
            for &r in pattern.iter().rev() {
                let j = pinv[r];
                if j == NONE {
                    continue;
                }
                let xr = x[r];
                if xr != 0.0 {
                    for &(i, l) in &lcols[j] {
                        x[i] -= l * xr;
                    }
                }
            }

            // Partial pivot among the rows not yet pivoted.
            let mut piv_row = NONE;
            let mut piv_mag = 0.0_f64;
            for &r in &pattern {
                if pinv[r] == NONE && x[r].abs() > piv_mag {
                    piv_mag = x[r].abs();
                    piv_row = r;
                }
            }
            if piv_row == NONE || piv_mag <= 1e-14 * scale {
                return None;
            }

            let pivot = x[piv_row];
            udiag[k] = pivot;
            prow[k] = piv_row;
            pinv[piv_row] = k;
            for &r in &pattern {
                if r == piv_row || x[r] == 0.0 {
                    continue;
                }
                if pinv[r] == NONE {
                    lcols[k].push((r, x[r] / pivot));
                } else {
                    ucols[k].push((pinv[r], x[r]));
                }
            }
            for &r in &pattern {
                x[r] = 0.0;
            }
        }

        Some(SparseLu {
            n,
            prow,
            lcols,
            ucols,
            udiag,
        })
    }

    /// Solve `A x = b` using the stored factors.
    fn solve(&self, b: &[f64]) -> Vec<f64> {
        let n = self.n;
        // Forward substitution with L, in original row indices.
        let mut y = b.to_vec();
        for k in 0..n {
            let yk = y[self.prow[k]];
            if yk != 0.0 {
                for &(i, l) in &self.lcols[k] {
                    y[i] -= l * yk;
                }
            }
        }
        // Back substitution with U, column-oriented, in pivot-step indices.
        let mut z: Vec<f64> = (0..n).map(|k| y[self.prow[k]]).collect();
        let mut x = vec![0.0; n];
        for k in (0..n).rev() {
            x[k] = z[k] / self.udiag[k];
            for &(i, u) in &self.ucols[k] {
                z[i] -= u * x[k];
            }
        }
        x
    }
}

/// Solve one SDIRK stage `k = f(t_stage, y_base + diag·k)` with the Newton
/// linear solves routed through the sparse CSR/LU path. Called by
/// [`sdirk_stage`](crate::linalg::sdirk_stage) whenever the system has at
/// least [`SPARSE_LU_MIN_N`] states, so the finite-difference Jacobian is
/// built directly in compressed storage and never densified.
pub(crate) fn sdirk_stage_sparse(
    f: &dyn crate::RhsCallable,
    t_stage: f64,
    y_base: &[f64],
    diag: f64,
    k_guess: &[f64],
    tol: Tol,
    jac: Option<&JacobianPattern>,
) -> Result<Vec<f64>, OdeError> {
    let n = y_base.len();
    let mut k = k_guess.to_vec();
    let mut corr = f64::INFINITY;
    for _iter in 0..50 {
        let y_stage: Vec<f64> = y_base.iter().zip(&k).map(|(b, kk)| b + diag * kk).collect();
        let mut fk = vec![0.0; n];
        f.call(t_stage, &y_stage, &mut fk)?;
        let r: Vec<f64> = k.iter().zip(&fk).map(|(kk, fv)| kk - fv).collect();
        let rnorm = r.iter().map(|x| x * x).sum::<f64>().sqrt();
        let jmat = match jac {
            Some(pattern) => CsrMatrix::jacobian_colored(
                |t, y, dydt| f.call(t, y, dydt),
                t_stage,
                &y_stage,
                &fk,
                pattern,
            )?,
            None => CsrMatrix::jacobian(
                |t, y, dydt| f.call(t, y, dydt),
                t_stage,
                &y_stage,
                &fk,
                None,
            )?,
        };
        // Newton matrix A = I - diag·J (sparse, since J is sparse).
        let a = jmat.scaled_identity_minus_scaled(diag);
        let delta = a.sparse_solve(&r).ok_or(OdeError::Newton {
            t: t_stage,
            residual: rnorm,
        })?;
        for (ki, di) in k.iter_mut().zip(&delta) {
            *ki -= di;
        }
        // The stage variable is a derivative; its effect on the state is diag·Δk.
        let state_step: Vec<f64> = delta.iter().map(|d| diag * d).collect();
        corr = tol.wrms(&state_step, &y_stage);
        if corr < NEWTON_TOL {
            return Ok(k);
        }
    }
    // Budget exhausted without convergence: report failure so the driver shrinks
    // the step instead of accepting an unconverged stage.
    Err(OdeError::Newton {
        t: t_stage,
        residual: corr,
    })
}

/// Structural sparsity of a Jacobian: for each row, the columns that may be
/// nonzero.
///
/// Supplying the pattern lets [`CsrMatrix::jacobian_colored`] differentiate
/// several structurally independent columns with a single RHS call, so the cost
/// of a Jacobian drops from `n` evaluations to the number of colors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JacobianPattern {
    n: usize,
    /// `rows[i]` is the sorted list of columns that can be nonzero in row `i`.
    rows: Vec<Vec<usize>>,
}

impl JacobianPattern {
    /// Build a pattern from per-row column lists. Each list is sorted and
    /// deduplicated.
    ///
    /// # Errors
    ///
    /// Returns [`OdeError::Invalid`] if any column index is out of range for
    /// `rows.len()` states.
    pub fn new(mut rows: Vec<Vec<usize>>) -> Result<Self, OdeError> {
        let n = rows.len();
        for row in &mut rows {
            row.sort_unstable();
            row.dedup();
            if let Some(&bad) = row.iter().find(|&&c| c >= n) {
                return Err(OdeError::invalid(format!(
                    "pattern column {bad} out of range for {n} states"
                )));
            }
        }
        Ok(Self { n, rows })
    }

    /// The fully dense pattern (every entry may be nonzero) for `n` states.
    #[must_use]
    pub fn dense(n: usize) -> Self {
        Self {
            n,
            rows: (0..n).map(|_| (0..n).collect()).collect(),
        }
    }

    /// Detect the pattern by probing `f`: column `j` is perturbed and every row
    /// whose value changes gets an entry. Costs `n + 1` RHS evaluations.
    ///
    /// Detection is exact for entries whose dependence is visible at `y`. An
    /// entry that is zero at this particular point (for example the product
    /// `x·y` when `x = 0`) is missed, so probe at a generic state when in doubt.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by the RHS `f`.
    pub fn detect<F>(f: F, t: f64, y: &[f64]) -> Result<Self, OdeError>
    where
        F: Fn(f64, &[f64], &mut [f64]) -> Result<(), OdeError>,
    {
        let n = y.len();
        let mut f0 = vec![0.0; n];
        f(t, y, &mut f0)?;
        let mut rows: Vec<Vec<usize>> = vec![Vec::new(); n];
        let sqrt_eps = f64::sqrt(f64::EPSILON);
        for col in 0..n {
            let dy = sqrt_eps * y[col].abs().max(1.0);
            let mut yp = y.to_vec();
            yp[col] += dy;
            let mut fp = vec![0.0; n];
            f(t, &yp, &mut fp)?;
            for (row, (a, b)) in fp.iter().zip(&f0).enumerate() {
                if a != b {
                    rows[row].push(col);
                }
            }
        }
        Self::new(rows)
    }

    /// Number of states.
    #[must_use]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Number of structurally nonzero entries.
    #[must_use]
    pub fn nnz(&self) -> usize {
        self.rows.iter().map(Vec::len).sum()
    }

    /// Columns that may be nonzero in row `i`.
    #[must_use]
    pub fn row(&self, i: usize) -> &[usize] {
        &self.rows[i]
    }

    /// Greedy column coloring: two columns get different colors whenever they
    /// share a row, so each color class can be perturbed together without the
    /// contributions colliding in any row.
    ///
    /// Returns `(colors, n_colors)`, where `colors[j]` is column `j`'s color.
    #[must_use]
    pub fn column_coloring(&self) -> (Vec<usize>, usize) {
        let mut conflicts: Vec<Vec<usize>> = vec![Vec::new(); self.n];
        for row in &self.rows {
            for &a in row {
                for &b in row {
                    if a != b {
                        conflicts[a].push(b);
                    }
                }
            }
        }
        let mut colors = vec![usize::MAX; self.n];
        let mut n_colors = 0usize;
        for j in 0..self.n {
            let mut used = vec![false; n_colors + 1];
            for &k in &conflicts[j] {
                if colors[k] != usize::MAX {
                    used[colors[k]] = true;
                }
            }
            let c = used.iter().position(|&u| !u).unwrap_or(n_colors);
            colors[j] = c;
            n_colors = n_colors.max(c + 1);
        }
        (colors, n_colors)
    }
}

impl CsrMatrix {
    /// Jacobian `J = df/dy` by forward differences, with one RHS call per color
    /// instead of one per column.
    ///
    /// Columns are grouped with [`JacobianPattern::column_coloring`]. All columns
    /// of a color are perturbed together, and each row's entry is recovered from
    /// the single column of that color that the row's pattern contains. The
    /// result equals [`CsrMatrix::jacobian`] when `pattern` covers the true
    /// structure.
    ///
    /// # Errors
    ///
    /// Propagates any error returned by the RHS `f`, or returns
    /// [`OdeError::Invalid`] if the pattern size does not match `y`.
    pub fn jacobian_colored<F>(
        f: F,
        t: f64,
        y: &[f64],
        f0: &[f64],
        pattern: &JacobianPattern,
    ) -> Result<CsrMatrix, OdeError>
    where
        F: Fn(f64, &[f64], &mut [f64]) -> Result<(), OdeError>,
    {
        let n = y.len();
        if pattern.n() != n {
            return Err(OdeError::invalid(format!(
                "pattern is for {} states, state has {n}",
                pattern.n()
            )));
        }
        let sqrt_eps = f64::sqrt(f64::EPSILON);
        let (colors, n_colors) = pattern.column_coloring();
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n_colors];
        for (j, &c) in colors.iter().enumerate() {
            members[c].push(j);
        }
        let mut dy = vec![0.0; n];
        let (mut rows, mut cols, mut vals) = (Vec::new(), Vec::new(), Vec::new());
        for (color, group) in members.iter().enumerate() {
            let mut yp = y.to_vec();
            for &j in group {
                dy[j] = sqrt_eps * y[j].abs().max(1.0);
                yp[j] += dy[j];
            }
            let mut fp = vec![0.0; n];
            f(t, &yp, &mut fp)?;
            for row in 0..n {
                // Conflict-freeness guarantees at most one column of this color
                // in each row's pattern.
                let hit = pattern
                    .row(row)
                    .iter()
                    .copied()
                    .find(|&j| colors[j] == color);
                if let Some(j) = hit {
                    let val = (fp[row] - f0[row]) / dy[j];
                    if val != 0.0 {
                        rows.push(row);
                        cols.push(j);
                        vals.push(val);
                    }
                }
            }
            for &j in group {
                dy[j] = 0.0;
            }
        }
        Ok(CsrMatrix::from_triplets(n, rows, cols, vals))
    }

    /// Compress `(row, col, value)` triplets into CSR, with each row's entries in
    /// ascending column order.
    fn from_triplets(n: usize, rows: Vec<usize>, cols: Vec<usize>, vals: Vec<f64>) -> CsrMatrix {
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| (rows[a], cols[a]).cmp(&(rows[b], cols[b])));
        let mut indptr = vec![0usize; n + 1];
        for &k in &order {
            indptr[rows[k] + 1] += 1;
        }
        for r in 0..n {
            indptr[r + 1] += indptr[r];
        }
        let mut indices = Vec::with_capacity(vals.len());
        let mut data = Vec::with_capacity(vals.len());
        for &k in &order {
            indices.push(cols[k]);
            data.push(vals[k]);
        }
        CsrMatrix {
            nrows: n,
            ncols: n,
            indptr,
            indices,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csr_mat_vec_matches_dense() {
        // A = [[2, 0, 1], [0, 3, 0], [1, 0, 4]]  (sparse, diagonally dominant)
        let a = CsrMatrix::from_dense(&[2.0, 0.0, 1.0, 0.0, 3.0, 0.0, 1.0, 0.0, 4.0], 3, 3);
        let y = a.mat_vec(&[1.0, 2.0, 3.0]);
        assert!((y[0] - 5.0).abs() < 1e-12);
        assert!((y[1] - 6.0).abs() < 1e-12);
        assert!((y[2] - 13.0).abs() < 1e-12);
    }

    #[test]
    fn sparse_solve_matches_dense_solve() {
        // Same matrix as the dense LU test in linalg.rs.
        let dense = [2.0, 1.0, 1.0, 1.0, 3.0, 2.0, 1.0, 0.0, 1.0];
        let a = CsrMatrix::from_dense(&dense, 3, 3);
        let b = [4.0, 6.0, 2.0];
        let x = a.sparse_solve(&b).expect("non-singular");
        assert!((x[0] - 1.0).abs() < 1e-12);
        assert!((x[1] - 1.0).abs() < 1e-12);
        assert!((x[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn sparse_solve_pivots_on_singular_offdiag() {
        // [0 1; 1 0] — pivot required at k=0.
        let a = CsrMatrix::from_dense(&[0.0, 1.0, 1.0, 0.0], 2, 2);
        let x = a
            .sparse_solve(&[3.0, 4.0])
            .expect("non-singular after pivot");
        assert!((x[0] - 4.0).abs() < 1e-12);
        assert!((x[1] - 3.0).abs() < 1e-12);
    }

    #[test]
    fn sparse_solve_detects_singular() {
        let a = CsrMatrix::from_dense(&[1.0, 1.0, 2.0, 2.0], 2, 2);
        assert!(a.sparse_solve(&[1.0, 1.0]).is_none());
        // An all-zero column has no pivot.
        let z = CsrMatrix::from_dense(&[1.0, 0.0, 2.0, 0.0], 2, 2);
        assert!(z.sparse_solve(&[1.0, 1.0]).is_none());
    }

    #[test]
    fn sparse_lu_matches_dense_on_banded_system_with_fill() {
        // A banded, non-symmetric system with a few long-range entries. The
        // long-range entries force fill and off-diagonal pivoting.
        const N: usize = 150;
        let mut dense = vec![0.0; N * N];
        for i in 0..N {
            dense[i * N + i] = 4.0 + (i % 7) as f64 * 0.25;
            for d in 1..=3usize {
                if i + d < N {
                    dense[i * N + i + d] = -1.0 + 0.1 * d as f64;
                }
                if i >= d {
                    dense[i * N + i - d] = 0.7 - 0.2 * d as f64;
                }
            }
        }
        dense[3 * N + 140] = 0.9;
        dense[140 * N + 3] = -1.1;
        dense[77 * N + 2] = 2.5; // large off-diagonal, pivoting needed
        let a = CsrMatrix::from_dense(&dense, N, N);
        let b: Vec<f64> = (0..N).map(|i| (i as f64 * 0.37).sin() + 0.5).collect();

        let x = a.sparse_solve(&b).expect("non-singular");
        let mut dm = crate::linalg::DMat::new(N, N);
        dm.data.copy_from_slice(&dense);
        let xd = dm.solve(&b).expect("non-singular");
        for (i, (xs, xdv)) in x.iter().zip(&xd).enumerate() {
            assert!((xs - xdv).abs() < 1e-9, "component {i}: {xs} vs {xdv}");
        }
        // Residual check, independent of the dense oracle.
        let ax = a.mat_vec(&x);
        for (axi, bi) in ax.iter().zip(&b) {
            assert!((axi - bi).abs() < 1e-9);
        }
    }

    #[test]
    fn jacobian_csr_of_linear_system() {
        let a = CsrMatrix::from_dense(&[0.0, 1.0, -2.0, -3.0], 2, 2);
        let dense_j = a.to_dense();
        let f = move |_t: f64, y: &[f64], dydt: &mut [f64]| {
            let out = a.mat_vec(y);
            dydt[0] = out[0];
            dydt[1] = out[1];
            Ok(())
        };
        let y = [0.7, -1.3];
        let mut f0 = [0.0; 2];
        f(0.0, &y, &mut f0).unwrap();
        let j = CsrMatrix::jacobian(&f, 0.0, &y, &f0, None).unwrap();
        assert!((j.get(0, 0) - 0.0).abs() < 1e-8);
        assert!((j.get(0, 1) - 1.0).abs() < 1e-8);
        assert!((j.get(1, 0) - (-2.0)).abs() < 1e-8);
        assert!((j.get(1, 1) - (-3.0)).abs() < 1e-8);
        // The CSR Newton matrix of (I - 0.1 J) must match the dense DMat path.
        let jac = CsrMatrix::from_dense(&dense_j, 2, 2);
        let csr = jac.scaled_identity_minus_scaled(0.1);
        let r = [1.5, -2.5];
        let xs = csr.sparse_solve(&r).unwrap();
        // Dense oracle: A = I - 0.1 J.
        let mut dense_a = crate::linalg::DMat::identity(2);
        for i in 0..2 {
            for jj in 0..2 {
                dense_a.set(i, jj, dense_a.get(i, jj) - 0.1 * dense_j[i * 2 + jj]);
            }
        }
        let xd = dense_a.solve(&r).unwrap();
        assert!((xs[0] - xd[0]).abs() < 1e-10);
        assert!((xs[1] - xd[1]).abs() < 1e-10);
    }

    #[test]
    fn bdf_sparse_path_solves_large_decoupled_system() {
        // n = 80 ≥ SPARSE_LU_MIN_N: `step_bdf` routes its Newton solves
        // through the sparse CSR/LU path. Decoupled decays have the closed
        // form y_i(t) = exp(-(i+1)·t).
        const N: usize = 80;
        let prob = crate::OdeProblem::new(
            |_t, y, dydt| {
                for (i, yi) in y.iter().enumerate() {
                    dydt[i] = -((i + 1) as f64) * yi;
                }
            },
            vec![1.0; N],
            0.0,
        )
        .unwrap();
        let t = 0.5;
        let y = prob.solve(crate::Method::Bdf, t).unwrap();
        for (i, yi) in y.iter().enumerate() {
            let expected = (-((i + 1) as f64) * t).exp();
            assert!(
                (yi - expected).abs() < 1e-6,
                "component {i}: {yi} vs {expected}"
            );
        }
    }

    #[test]
    fn scaled_identity_handles_missing_diagonal() {
        // J with a structural zero on the diagonal: A = I - diag·J must still
        // carry the unit diagonal entry there.
        let j = CsrMatrix::from_dense(&[0.0, 2.0, 0.0, 0.0], 2, 2);
        let a = j.scaled_identity_minus_scaled(0.5);
        assert!((a.get(0, 0) - 1.0).abs() < 1e-14);
        assert!((a.get(0, 1) - (-1.0)).abs() < 1e-14);
        assert!((a.get(1, 0) - 0.0).abs() < 1e-14);
        assert!((a.get(1, 1) - 1.0).abs() < 1e-14);
        let x = a.sparse_solve(&[1.0, 2.0]).unwrap();
        // x0 - x1 = 1, x1 = 2  ->  x = [3, 2].
        assert!((x[0] - 3.0).abs() < 1e-12);
        assert!((x[1] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn sparse_sdirk_stage_matches_dense_oracle_on_tridiagonal_system() {
        // f(y) = A y with a tridiagonal A (n = 80 ≥ SPARSE_LU_MIN_N): the
        // stage equation k = A(y + diag·k) has the closed form
        // k = (I - diag·A)⁻¹ A y, computable with the dense DMat oracle.
        const N: usize = 80;
        const TOL: Tol = Tol {
            rtol: 1e-6,
            atol: 1e-6,
        };
        let mut dense = vec![0.0; N * N];
        for i in 0..N {
            dense[i * N + i] = -2.0;
            if i > 0 {
                dense[i * N + i - 1] = 1.0;
            }
            if i + 1 < N {
                dense[i * N + i + 1] = 1.0;
            }
        }
        let a = CsrMatrix::from_dense(&dense, N, N);
        let f = move |_t: f64, y: &[f64], dydt: &mut [f64]| {
            let out = a.mat_vec(y);
            dydt.copy_from_slice(&out);
        };
        let y: Vec<f64> = (0..N).map(|i| 0.1 * (i as f64).sin() + 0.2).collect();
        let diag = 0.05;
        let k = sdirk_stage_sparse(&f, 0.0, &y, diag, &vec![0.0; N], TOL, None).unwrap();
        // Dense oracle.
        let mut am = crate::linalg::DMat::new(N, N);
        for i in 0..N {
            for jj in 0..N {
                am.set(i, jj, dense[i * N + jj]);
            }
        }
        let ay = am.mat_vec(&y);
        // (I - diag·A) as a fresh dense matrix.
        let mut m = crate::linalg::DMat::new(N, N);
        for i in 0..N {
            for jj in 0..N {
                let v = if i == jj { 1.0 } else { 0.0 } - diag * dense[i * N + jj];
                m.set(i, jj, v);
            }
        }
        let expected = m.solve(&ay).unwrap();
        for (i, (kv, ev)) in k.iter().zip(&expected).enumerate() {
            assert!((kv - ev).abs() < 1e-8, "component {i}: {kv} vs {ev}");
        }
    }
}
