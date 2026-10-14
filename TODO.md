# TPT Research OS — TODO

Legend: `[x]` done · `[~]` partly done · `[ ]` open. Section numbers refer to [spec.txt](spec.txt).

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
- [x] Write `docs/ARCHITECTURE.md` (crate map, data flow, pinned foundation revs — revs empty until §1 spikes)

## 1. Foundation-crate spikes (confirm names, APIs, licences — spec §5)
- [~] `tpt-keystone-db`: open a local DB, store/query a record ✓ (builds on Windows; used by research-store), vector search not yet tried. Gotchas: `key` is a reserved word in its SQL; its lease has no release API and does not block a second handle with the same holder id, so research-store takes an OS file lock instead.
- [~] `tpt-math` pinned at GitHub master (a16a8f5): `linalg-dense` (solve) and `prob-dist` (normal) build and are bound in research-compute. Units, autodiff, arbitrary precision and `tpt-mathr` not yet checked
- [x] `tpt-science`: ODE solve end to end. `tpt-sci-ode` vendored at `vendor/tpt-sci-ode` without the cranelift JIT, which avoids the version issue. Upstream repo cannot be pinned by git: its `templates/` manifests are not valid TOML and break cargo. Raise upstream
- [ ] `tpt-chemistry`, `tpt-fem`: confirm they build; decide MVP exposure (spec §10)
- [~] `tpt-appfront`: pinned at GitHub master (6146526). The canvas backend builds on Windows and `research-app` opens a native window with a heading, status text and button. Not yet run interactively. Text editor and plot widgets not checked
- [ ] `tpt-anima` (light subset): can it be used without model downloads?
- [~] `tpt-primitives` pinned at its GitHub master (55b4a2a); capability API used by research-agents and tested. `tpt-crypto` not yet checked
- [x] Licence audit with `cargo deny`: licences, bans and sources pass for the current tree (incl. Keystone)
- [~] Add each confirmed crate to `[workspace.dependencies]` with a pinned rev (done: `tpt-keystone`, `tpt-primitives`)

## 2. research-core
- [x] `ProjectId`, `RunId`, `DocId`, `ResultId` types (validated `Id<T>` newtypes; generation not yet implemented)
- [x] Unified error type
- [x] Project manifest schema (name, created, format version, foundation revs)

## 3. research-store (Keystone)
- [x] Project database layout: `runs`, `results`, `documents`, `meta` tables; blobs under `blobs/`. Run inputs hold parameters; blobs hold files. Schema v2
- [x] Open / create / close a project directory (close = drop; OS lock released with it)
- [x] Experiment tracking: run record with inputs, code hash, environment, timings
- [x] Result storage: scalars, arrays, tables, text, and blobs (plots and binary data, content-addressed, hash re-checked on read)
- [~] Query API: list, filter, runs-by-input. Compare view waits for the UI (§6)
- [x] Schema versioning and migrations: v2 adds documents; v1 stores upgrade on open (tested). Future bumps add a match arm in `apply_migration`
- [~] Crash-safety and concurrent-open tests: concurrent open refused, manifest and blob writes atomic, reopen survives. No kill-the-process test yet

## 4. research-doc (computational documents)
- [x] Format spec in `docs/DOCUMENT_FORMAT.md`: cells (code, markdown, output), metadata, stable cell ids
- [x] Serialize / deserialize with deterministic output (diff-friendly, versionable)
- [~] `.ipynb` import (code, markdown, outputs; flags cells outside the hosted-language list). The list is a parameter; the language itself is still open in §5
- [x] `.ipynb` export (nbformat 4.5)
- [x] Round-trip tests (JSON exact; document → notebook → document). Tables come back as text, by design

## 5. research-compute
- [x] Choose the cell language: Rhai (decision and reasons in `docs/ARCHITECTURE.md`; `tpt-mathr` is a numeric library, not a language)
- [~] Bind `tpt-math` units / linalg / probability / autodiff: `solve` (linalg) and `normal` (probability) bound and tested. Units and autodiff not yet
- [~] Bind `tpt-science` ODE solvers: `ode_solve` runs the four explicit and implicit methods from Rhai, tested (exponential decay, parameters, stiff problem, bad inputs). DAE, events and sensitivities not yet
- [~] Cell execution and re-run of stale cells: cells run in order on a shared scope, a changed cell reruns with everything after it. A true dependency graph is not built
- [x] Cancellation, timeouts, memory limit: cancel flag, wall-clock timeout, operation, string and collection limits. All tested
- [x] Deterministic seeding for reproducibility: `uniform(index)` from the session seed, tested across sessions
- [~] Output capture: text and errors captured. Tables and plots not yet (plots are blobs in research-store, so the path exists)
- [x] Record every run into research-store: each cell that runs becomes a run, with its outputs and value as results

## 6. research-app (AppFront UI)
- [~] Main window: opens the project in the directory given on the command line (created if missing). Starts and stays running; not yet checked by clicking through. No open/create dialog
- [ ] Project browser (documents, runs, data)
- [~] Document editor: documents list, open/new, cell source edits, add code and text cells, run, save, and outputs shown as text. Headless logic tested (`editor.rs`, `view.rs`). Narrative rendering of text cells is plain text; no inline plots or tables yet
- [ ] Plot viewer
- [ ] Experiment/run comparison view
- [ ] Settings (agents on/off, paths)
- [ ] Keyboard shortcuts, light/dark theme
- [ ] Decide MVP exposure of library surface vs library-only (spec §10)

## 7. research-package (spec §4, §6)
- [x] Package layout: zip with manifest, DB snapshot, documents, data files (whole project directory; `docs/PACKAGE_FORMAT.md`)
- [~] Export / import round-trip: tested into another directory with a store reopen. Not yet tested across two machines
- [x] Integrity: per-file hashes in manifest
- [x] ed25519 signing and verification
- [x] Optional encryption on export (chacha20poly1305 + argon2, as in digital-twin)
- [x] Reject malformed or malicious archives: traversal, reserved names, case collisions, unlisted or duplicate entries, size lies, oversized totals. Zip-bomb reads are bounded by declared size
- [x] Version compatibility checks (package and project format versions)
- [ ] Packing reads the whole project into memory. Streaming needed for large projects

## 8. research-agents (optional, light)
- [x] Capability model using `tpt-primitives` (deny by default): agent starts with no grants, stopped by default, every attempt logged. Lease and epoch bindings are denied until the app supplies them
- [x] Reproducibility check (re-run and compare): `research_agents::check_document` reruns a copy from scratch with the same seed and reports per-cell matches, differences and never-run cells. `check_with_agent` requires a `document:<id>` `rerun` grant. Checks documents, not runs across machines; no UI wiring yet
- [ ] Analysis suggestion helper
- [x] No large model download by default (spec §8): nothing in the crate downloads anything
- [~] Full action log: done (`Agent::log`). UI indicator: `Agent::is_running` exists; the indicator itself is §6

## 9. research-license + distribution (spec §7)
- [~] Offline licence key file verification (no network, no telemetry): Ed25519 check against a supplied vendor key, tested. The vendor public key is not embedded yet, and the vendor signing tool is kept out of this repo
- [x] Open-source build works with no licence file: `Status::NotProvided` is not an error (app wiring in §6)
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
