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
- [~] `tpt-math` pinned at GitHub master (a16a8f5): `linalg-dense` (solve), `prob-dist` (normal) and `units-dyn` (dimension-checked quantities) build and are bound in research-compute. Autodiff: forward mode (`tpt-math-autodiff-fwd`, same rev) is bound as `diff` and tested. Reverse mode is not needed yet. Arbitrary precision: not used, since nothing needs it yet. `tpt-mathr` is early-stage with an unstable API and no performance claims, so it stays out until it stabilises
- [x] `tpt-science`: ODE solve end to end. `tpt-sci-ode` vendored at `vendor/tpt-sci-ode` without the cranelift JIT, which avoids the version issue. Upstream repo cannot be pinned by git: its `templates/` manifests are not valid TOML and break cargo. Raise upstream
- [~] `tpt-chemistry` (7c420ce857244b8fbab214b1520dcebbda8d879b) and `tpt-fem` (366512b3cf1dd4a59dcb2f3a1ed6874eb3e72eb3): both facade crates build on Windows (scratch probe, `cargo check`). Not added to the workspace, and not pinned yet. Decision: neither is in the MVP, so they stay out of the workspace until §11 "Deeper chemistry and FEM integration" is picked up
- [~] `tpt-appfront`: pinned at GitHub master (6146526). The canvas backend builds on Windows and `research-app` opens a native window with a heading, status text and button. Not yet run interactively. Text editor and plot widgets not checked
- [x] `tpt-anima` (light subset): the umbrella crate builds on Windows, and no crate in its dependency tree downloads models. The network code (`tpt-anima-edge-gateway`, reqwest) is not a dependency of the umbrella. Decision: no anima dependency for now; agents stay on `tpt-primitives`, which already works
- [~] `tpt-primitives` pinned at its GitHub master (55b4a2a); capability API used by research-agents and tested. `tpt-crypto` builds on Windows, but package signing and encryption already use the audited crates.io crates (ed25519-dalek, chacha20poly1305, argon2). Decision: keep those and do not adopt `tpt-crypto` for MVP
- [x] Licence audit with `cargo deny`: licences, bans and sources pass for the current tree (incl. Keystone)
- [~] Add each confirmed crate to `[workspace.dependencies]` with a pinned rev (done: `tpt-keystone`, `tpt-primitives`)

## 2. research-core
- [x] `ProjectId`, `RunId`, `DocId`, `ResultId` types (validated `Id<T>` newtypes; `Id::generate(prefix)` makes a random 128-bit id)
- [x] Unified error type
- [x] Project manifest schema (name, created, format version, foundation revs)

## 3. research-store (Keystone)
- [x] Project database layout: `runs`, `results`, `documents`, `meta` tables; blobs under `blobs/`. Run inputs hold parameters; blobs hold files. Schema v2
- [x] Open / create / close a project directory (close = drop; OS lock released with it)
- [x] Experiment tracking: run record with inputs, code hash, environment, timings
- [x] Result storage: scalars, arrays, tables, text, and blobs (plots and binary data, content-addressed, hash re-checked on read)
- [x] Query API: list, filter, runs-by-input, and `compare_runs` (inputs that differ, results per run). The comparison window is in §6
- [x] Schema versioning and migrations: v2 adds documents; v1 stores upgrade on open (tested). Future bumps add a match arm in `apply_migration`
- [x] Crash-safety and concurrent-open tests: concurrent open refused, manifest and blob writes atomic, reopen survives, and a child process that aborts mid-write leaves a project that reopens with every completed run (`tests/crash.rs`)

## 4. research-doc (computational documents)
- [x] Format spec in `docs/DOCUMENT_FORMAT.md`: cells (code, markdown, output), metadata, stable cell ids. Format version stays 1 until the first release, since no reader exists yet; the `svg` output is part of it. The version is bumped from the first release on
- [x] Serialize / deserialize with deterministic output (diff-friendly, versionable)
- [~] `.ipynb` import (code, markdown, outputs; flags cells outside the hosted-language list). The list is a parameter, and `research_doc::HOSTED_LANGUAGES` now holds the one language the compute layer runs (`rhai`), checked against the compute crate in a test
- [x] `.ipynb` export (nbformat 4.5)
- [x] Round-trip tests (JSON exact; document → notebook → document). Tables come back as text, by design

## 5. research-compute
- [x] Choose the cell language: Rhai (decision and reasons in `docs/ARCHITECTURE.md`; `tpt-mathr` is a numeric library, not a language)
- [~] Bind `tpt-math` units / linalg / probability / autodiff: units (`qty`, `qty_in`, dimension-checked `+ - * /`), `solve` (linalg) and `normal` (probability) bound and tested. Autodiff: `diff(expr, x)` and `diff(expr, x, params)` (forward mode, exact to rounding) bound and tested. DAE, events and sensitivities are still open for ODE
- [~] Bind `tpt-science` ODE solvers: `ode_solve` runs the four explicit and implicit methods from Rhai, tested (exponential decay, parameters, stiff problem, bad inputs). DAE, events and sensitivities not yet
- [~] Cell execution and re-run of stale cells: cells run in order on a shared scope, a changed cell reruns with everything after it. A true dependency graph is not built. After a reopen the first run reruns every cell, by design: the Rhai scope is not saved, so reusing outputs would leave later cells without the variables they read (test `after_reopen_a_changed_cell_still_sees_earlier_variables`). Changing this needs a saved scope
- [x] Cancellation, timeouts, memory limit: cancel flag, wall-clock timeout, operation, string and collection limits. All tested
- [x] Deterministic seeding for reproducibility: `uniform(index)` from the session seed, tested across sessions
- [x] Output capture: text, errors, tables (`show_table(columns, rows)`) and plots (`show_plot(svg)`, which must start with `<svg`). A recorded plot is stored as a blob with media type `image/svg+xml`, and the notebook export and import keep it as SVG. The window shows a plot as a size line, not an image; a rendered view would need AppFront image support
- [x] Record every run into research-store: each cell that runs becomes a run, with its outputs and value as results

## 6. research-app (AppFront UI)
- [~] Main window: opens the project in the directory given on the command line (created if missing). Starts and stays running; not yet checked by clicking through. No open/create dialog
- [ ] Project browser (documents, runs, data)
- [~] Document editor: documents list, open/new, cell source edits, add code and text cells, run, save, and outputs shown as text. Headless logic tested (`editor.rs`, `view.rs`). Narrative rendering of text cells is plain text; no inline plots or tables yet
- [ ] Plot viewer
- [~] Experiment/run comparison view: tick runs in the side list, compare them side by side (differing inputs, results per run, missing results shown as —). Headless logic tested (`compare.rs`, `view.rs`). Not yet checked in the window; plot results not shown
- [x] Settings: agents on/off, saved with the project as `app.agents` (`ProjectStore::set_setting`, tested). Off by default; "Check reproducibility" needs it on. Paths: none to set yet
- [ ] Keyboard shortcuts, light/dark theme: blocked upstream. `tpt-appfront-canvas` passes no key events to the app, and `run_native` gives no hook to set the egui visuals, so the app follows the egui default
- [x] Decide MVP exposure of library surface vs library-only (spec §10). Decision: the MVP UI exposes documents, runs, comparison and packaging only. Library-only: the `tpt-math` bindings beyond units/solve/normal, DAE/events, agents' suggestion helper, chemistry and FEM

## 7. research-package (spec §4, §6)
- [x] Package layout: zip with manifest, DB snapshot, documents, data files (whole project directory; `docs/PACKAGE_FORMAT.md`)
- [~] Export / import round-trip: tested into another directory with a store reopen. Not yet tested across two machines
- [x] Integrity: per-file hashes in manifest
- [x] ed25519 signing and verification
- [x] Optional encryption on export (chacha20poly1305 + argon2, as in digital-twin)
- [x] Reject malformed or malicious archives: traversal, reserved names, case collisions, unlisted or duplicate entries, size lies, oversized totals. Zip-bomb reads are bounded by declared size
- [x] Version compatibility checks (package and project format versions)
- [x] Packing: decision is no streaming for the MVP. Files are hashed and written from disk, each checked against its hash as it is written (`build_zip_checked`, tested). Measured in `docs/PACKAGE_FORMAT.md`: peak memory is about 1.7 to 2 times the project size, and 800 MiB packs in about 25 s (release). Import is in memory by design, and refuses more than 2 GiB, which is the real package limit. Streamed import or encryption would be needed to go beyond that

## 8. research-agents (optional, light)
- [x] Capability model using `tpt-primitives` (deny by default): agent starts with no grants, stopped by default, every attempt logged. Lease and epoch bindings are denied until the app supplies them
- [x] Reproducibility check (re-run and compare): `research_agents::check_document` reruns a copy from scratch with the same seed and reports per-cell matches, differences and never-run cells. `check_with_agent` requires a `document:<id>` `rerun` grant. Checks documents, not runs across machines. Wired into the window as "Check reproducibility" (needs agents on); each check uses a fresh agent holding only the rerun grant
- [ ] Analysis suggestion helper. Decision: deferred. The spec does not define what it suggests, so it needs a spec before any code
- [x] No large model download by default (spec §8): nothing in the crate downloads anything
- [~] Full action log: done (`Agent::log`). UI indicator: `Agent::is_running` exists, but checks run synchronously and stop the agent, so there is nothing to show while one runs. The attempts are kept in `Editor::agent_log`

## 9. research-license + distribution (spec §7)
- [~] Offline licence key file verification (no network, no telemetry): Ed25519 check against a supplied vendor key, tested. The vendor public key is not embedded yet, and the vendor signing tool is kept out of this repo
- [x] Open-source build works with no licence file: `Status::NotProvided` is not an error (app wiring in §6)
- [ ] Release builds for Windows, macOS, Linux
- [ ] Code signing and installers (needs certificates)
- [~] Bundle example research projects: `cargo run -p research-app --example demo_project -- <dir>` builds a project with an ODE, a units cell, sampling and a linear solve, all run. Not yet packaged into the release zip
- [x] Quick-start guide: `docs/QUICKSTART.md`, with `docs/QUICKSTART.pdf` rendered from it. Regenerate the PDF after editing the Markdown
- [ ] Finalize [GUMROAD.md](GUMROAD.md) listing and publish
- [ ] Error-report submission process for buyers

## 10. Quality and release gates
- [~] Verify no network calls anywhere: static gate added (`deny.toml` bans HTTP client crates). Not enough alone: Keystone (`tpt-keystone`) links tokio, rustls and mio, and its wire, mcp, mirror and synapse modules compile into the library. Runtime check: the editor, launched and left running for 4 s (including Keystone startup), had no TCP or UDP endpoints. A 60 s idle run also had no TCP or UDP endpoints. Still open: a session that runs cells and saves, and a firewall run. Upstream should gate those modules behind a feature
- [~] Verify no telemetry: static gate added (`deny.toml` bans opentelemetry and sentry). Keystone's OTLP exporter is off in this build. Runtime check: no extra endpoints seen, as above
- [~] Success-criteria walk-through (spec §9): create project → run workflow → store results → move to another machine. Tested on one machine (`research-app/tests/walkthrough.rs`): pack, unpack into another directory, reopen, runs and outputs match. A real second machine is still open
- [ ] Reproducibility test: same project, two machines, identical results
- [~] Performance check: measured on the development PC, not a mid-range laptop (release build: 800 MiB pack 25 s, unpack 10 s, peak 1.6 GiB). Laptop run still open
- [x] Security review of package import and agent sandbox: no exploitable issue found. Lease-bound capabilities are tested as denied (no lease is supplied). Decisions: the agent action log stays unbounded, because each entry is the audit record and agents only run on user action; a log cap would need its own design. Import holds the archive in memory (see §7), by design
- [x] README, CONTRIBUTING, SECURITY docs. CI now runs `cargo deny check bans licenses sources`

## 11. Later (no commitment, spec §4)
- [ ] Deeper chemistry and FEM integration
- [ ] Stronger agents
- [ ] LAN distributed compute
- [ ] Reproducibility certificates / formal claims
- [ ] Headless / batch mode
