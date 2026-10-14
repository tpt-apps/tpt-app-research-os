# Vendored: tpt-sci-ode

- Source: https://github.com/tpt-solutions/tpt-science, `crates/tpt-sci-ode`, commit `7c5d0dea29ee11d844da93487b4453f582a79f38` (master, 2026-10-15).
- Licence: MIT OR Apache-2.0 (copied `LICENSE-MIT` and `LICENSE-APACHE`).

## Why vendored

Cargo scans every manifest in a git repository while it looks for a package. The upstream repo has `templates/*/Cargo.toml` files with placeholder syntax (`{{tpt_sci_spec}}`) that are not valid TOML, so `tpt-sci-ode` cannot be pinned by git. This is the same reason digital-twin vendors `tpt-perception`.

## Changes from upstream

- Removed the `cranelift-jit` feature and `src/jit.rs`, so the crate has no cranelift dependency and no `unsafe` JIT code. The default feature is now empty.
- Replaced workspace-inherited fields with explicit values, and dropped the workspace lint table.
- Dropped the `diffsol` verification-oracle feature and the benches, examples and tests. Those are not needed at runtime. The solver's behaviour is checked through research-compute's tests.
- Depends on `tpt-math-numeric` 0.1.0 from crates.io, as upstream does.

Do not edit `src/` here. Re-vendor from upstream and apply the same changes.
