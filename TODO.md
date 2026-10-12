# TPT Research OS — TODO

Legend: `[x]` done · `[ ]` open. Section numbers refer to [spec.txt](spec.txt).

## Decisions made
- [x] Foundation crates come from GitHub (`TPT-Solutions/*`), pinned by `rev` like `tpt-app-digital-twin`
- [x] Document format: custom native format, plus `.ipynb` import/export; cells run in a Rust-hosted language only
- [x] Shell: pure AppFront native (no Tauri); package = zip + manifest + ed25519 signature + optional encryption
- [x] Session goal: TODO + workspace scaffold

## 0. Scaffold
- [x] Cargo workspace with 8 crates (core, store, doc, compute, package, agents, license, app)
- [x] LICENSE-MIT / LICENSE-APACHE, rust-toolchain, deny.toml, CI workflow
- [x] `cargo test --workspace` passes on the empty skeleton
- [ ] `git init`, first commit, create GitHub repo, push
- [ ] Verify CI is green on Windows / macOS / Linux
- [ ] Write `docs/ARCHITECTURE.md` (crate map, data flow, pinned foundation revs)

## 1. Foundation-crate spikes (confirm names, APIs, licences — spec §5)
- [ ] `tpt-keystone-db`: open a local DB, store/query a record, vector search
- [ ] `tpt-math` (local checkout exists) + `tpt-mathr`: units, linalg, autodiff, arbitrary precision
- [ ] `tpt-science`: ODE solve end to end (watch the cranelift version issue seen in digital-twin's vendored `tpt-sci-ode`)
- [ ] `tpt-chemistry`, `tpt-fem`: confirm they build; decide MVP exposure (spec §10)
- [ ] `tpt-appfront`: window, text editor widget, plot/canvas rendering
- [ ] `tpt-anima` (light subset): can it be used without model downloads?
- [ ] `tpt-primitives`, `tpt-crypto`: identity / capability / evidence types
- [ ] Licence audit with `cargo deny`: every dependency MIT-and-Apache compatible (no Apache-only)
- [ ] Add each confirmed crate to `[workspace.dependencies]` with a pinned rev

## 2. research-core
- [ ] `ProjectId`, `RunId`, `DocId`, `ResultId` types
- [ ] Unified error type
- [ ] Project manifest schema (name, created, format version, foundation revs)

## 3. research-store (Keystone)
- [ ] Project database layout (documents, runs, results, parameters, files)
- [ ] Open / create / close a project directory
- [ ] Experiment tracking: run record with inputs, code hash, environment, timings
- [ ] Result storage: scalars, arrays, tables, plots, binary blobs
- [ ] Query API: list/filter/compare runs
- [ ] Schema versioning and migrations
- [ ] Crash-safety and concurrent-open tests

## 4. research-doc (computational documents)
- [ ] Format spec in `docs/DOCUMENT_FORMAT.md`: cells (code, markdown, output), metadata, stable cell ids
- [ ] Serialize / deserialize with deterministic output (diff-friendly, versionable)
- [ ] `.ipynb` import (code, markdown, outputs; flag non-Rust-hosted-language cells)
- [ ] `.ipynb` export
- [ ] Round-trip tests

## 5. research-compute
- [ ] Choose the cell language (Rhai, `tpt-mathr`, or other) via spike
- [ ] Bind `tpt-math` units / linalg / probability / autodiff
- [ ] Bind `tpt-science` ODE/DAE solvers
- [ ] Cell execution with dependency graph and re-run of stale cells
- [ ] Cancellation, timeouts, memory limit
- [ ] Deterministic seeding for reproducibility
- [ ] Output capture: text, tables, plots
- [ ] Record every run into research-store

## 6. research-app (AppFront UI)
- [ ] Main window, project open/create dialogs
- [ ] Project browser (documents, runs, data)
- [ ] Document editor: cells, run buttons, inline results, narrative rendering
- [ ] Plot viewer
- [ ] Experiment/run comparison view
- [ ] Settings (agents on/off, paths)
- [ ] Keyboard shortcuts, light/dark theme
- [ ] Decide MVP exposure of library surface vs library-only (spec §10)

## 7. research-package (spec §4, §6)
- [ ] Package layout: zip with manifest, DB snapshot, documents, data files
- [ ] Export / import round-trip to another machine
- [ ] Integrity: per-file hashes in manifest
- [ ] ed25519 signing and verification
- [ ] Optional encryption on export (chacha20poly1305 + argon2, as in digital-twin)
- [ ] Reject malformed or malicious archives (path traversal, zip bombs)
- [ ] Version compatibility checks

## 8. research-agents (optional, light)
- [ ] Capability model using `tpt-primitives` (deny by default)
- [ ] Reproducibility check agent (re-run and compare)
- [ ] Analysis suggestion helper
- [ ] No large model download by default (spec §8)
- [ ] Clear UI indicator when an agent is active; full action log

## 9. research-license + distribution (spec §7)
- [ ] Offline licence key file verification (no network, no telemetry)
- [ ] Open-source build works with no licence file
- [ ] Release builds for Windows, macOS, Linux
- [ ] Code signing and installers (needs certificates)
- [ ] Bundle example research projects
- [ ] Quick-start guide (PDF)
- [ ] Finalize [GUMROAD.md](GUMROAD.md) listing and publish
- [ ] Error-report submission process for buyers

## 10. Quality and release gates
- [ ] Verify no network calls anywhere (offline test, firewall run)
- [ ] Verify no telemetry
- [ ] Success-criteria walk-through (spec §9): create project → run workflow → store results → move to another machine
- [ ] Reproducibility test: same project, two machines, identical results
- [ ] Performance check on a mid-range laptop
- [ ] Security review of package import and agent sandbox
- [ ] README, CONTRIBUTING, SECURITY docs

## 11. Later (no commitment, spec §4)
- [ ] Deeper chemistry and FEM integration
- [ ] Stronger agents
- [ ] LAN distributed compute
- [ ] Reproducibility certificates / formal claims
- [ ] Headless / batch mode
