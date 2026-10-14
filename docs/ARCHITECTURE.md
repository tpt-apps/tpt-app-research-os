# Architecture

Status: the core crates and the editor window are built. Open work is in TODO.md.
Section numbers refer to [spec.txt](../spec.txt); open work is tracked in [TODO.md](../TODO.md).

## Crate map

| Crate | Responsibility | Main dependencies | State |
|-------|----------------|-------------------|-------|
| `research-core` | Typed ids, unified error, project manifest | serde, thiserror | Built, tested |
| `research-store` | Projects, runs, results, documents, blobs; schema v2 | research-core, research-doc, tpt-keystone | Built, tested; kill-mid-write test |
| `research-doc` | Document model, deterministic JSON, `.ipynb` import and export | research-core | Built, tested |
| `research-compute` | Rhai cells, sandbox, numeric bindings (`normal`, `solve`, `ode_solve`, units, `show_table`) | rhai, tpt-math crates, vendored tpt-sci-ode | Built, tested |
| `research-package` | Signed, optionally encrypted packages | ed25519-dalek, zip, argon2, chacha20poly1305 | Built, tested |
| `research-agents` | Capability-restricted agent; reproducibility check | tpt-primitives, research-compute | Built, tested |
| `research-license` | Offline licence key verification | ed25519-dalek, base64 | Built, tested; vendor key not embedded |
| `research-app` | Workspace model, editor view, window | tpt-appfront-canvas | Built; window not verified interactively |

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

## Cell language (decided)

Code cells are written in **Rhai** (`rhai` 1.x, MIT OR Apache-2.0), embedded in `research-compute`.

Why Rhai:

- It is pure Rust and dual-licensed, so it meets spec §5 with no extra licence work.
- It has no file, network or process access unless the host registers such functions. The engine registers only the research functions below.
- It has built-in limits for operations, string and collection size, and call depth, which cover the sandboxing and memory limits the spec needs (TODO §5).
- Cancellation and timeouts are a progress callback, so no OS thread is killed.

Rejected:

- `tpt-mathr` is a numeric library, not a language. It is used from Rhai as functions, not as the cell language itself.
- Python or another external interpreter would break the Rust-hosted rule and the sandbox.

Registered functions: `uniform(index)` returns a deterministic value in `[0, 1)` from the session seed, so runs reproduce. `tpt-math` and `tpt-science` bindings are added as more Rhai functions once those crates are pinned (TODO §1).
