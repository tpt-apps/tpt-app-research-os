# Architecture

Status: scaffold. Crate boundaries are set; most crates are still empty stubs.
Section numbers refer to [spec.txt](../spec.txt); open work is tracked in [TODO.md](../TODO.md).

## Crate map

| Crate | Responsibility | Depends on (planned) | State |
|-------|----------------|----------------------|-------|
| `research-core` | Typed ids, unified error, project manifest schema | serde, serde_json, thiserror | Types and tests in place |
| `research-store` | Project directory and Keystone database; run and result tracking | research-core, tpt-keystone-db | Stub |
| `research-doc` | Computational document format; `.ipynb` import/export | research-core | Stub |
| `research-compute` | Cell execution, dependency graph, output capture | research-core, research-store, research-doc, tpt-math, tpt-science | Stub |
| `research-package` | Zip packages: manifest, DB snapshot, documents, data; signing; encryption | research-core, research-store | Stub |
| `research-agents` | Optional, capability-restricted helpers | research-core, tpt-primitives | Stub |
| `research-license` | Offline licence key verification | research-core, tpt-crypto | Stub |
| `research-app` | AppFront native shell and UI | all of the above, tpt-appfront | Stub (`main.rs`) |

Dependency direction is one-way: `research-app` sits on top; `research-core` sits at the bottom
and depends on no other research crate. Nothing below `research-app` may depend on the UI.

## Core types (`research-core`)

- `Id<T>` is a validated string tagged by kind. `ProjectId`, `RunId`, `DocId` and `ResultId`
  are aliases of it, so one kind of id cannot be passed where another is expected.
  Ids are restricted to ASCII letters, digits, `-` and `_`, up to 128 characters, so they
  are always safe as file or directory names.
- `ProjectManifest` is the `manifest.json` schema: `format_version`, `id`, `name`,
  `created_unix`, and `foundation_revs` (a sorted map of crate name to git rev).
  Loading rejects manifests written by a newer format version than this build supports.
- `Error` is the shared error enum for these types.

## Data flow

```
 AppFront UI (research-app)
      │  user actions: open project, edit cell, run
      ▼
 research-doc ──parse──▶ cells ──▶ research-compute
                                    │  executes via tpt-math / tpt-science
                                    │  captures text, tables, plots
                                    ▼
                              research-store ──▶ project directory
                                    │             ├─ tpt-keystone-db (runs, results, params)
                                    │             ├─ documents/
                                    │             └─ manifest.json
                                    ▼
                              research-package ──▶ .zip package
                                                   manifest + DB snapshot + docs + data
                                                   per-file hashes, ed25519 signature,
                                                   optional encryption
```

Every computation run is recorded in `research-store` before its results are shown, so a result
on screen always has a run record behind it.

## Persistence and portability

- A project is a directory: `manifest.json`, the Keystone database, documents, and data files.
- Exporting produces a single package. Importing it on another machine must reproduce the same
  project; the manifest's `foundation_revs` records the dependency versions the project was
  last written with.
- Package import is untrusted input. It must reject path traversal, oversized archives
  (zip bombs) and bad signatures before anything is written to disk.

## Foundation crates

These come from `github.com/TPT-Solutions/*` and are added to `[workspace.dependencies]`
pinned by `rev`, once each spike in [TODO.md](../TODO.md) §1 confirms the crate name and API.
No revs are pinned yet.

Licence rule (spec §5): every dependency must be compatible with both MIT and Apache-2.0.
`deny.toml` enforces this in CI.

## Security boundaries

- No network access anywhere in the application. Verification is a release gate (TODO §10).
- Agents, when enabled, run with a deny-by-default capability set from `tpt-primitives`.
  The UI shows when an agent is active, and every agent action is logged.
- Cells run in a Rust-hosted language only; no shell or external process execution from
  notebook cells.
