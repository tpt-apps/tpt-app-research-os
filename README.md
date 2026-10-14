# TPT Research OS

An offline desktop environment for scientific and engineering research computing.
Projects hold documents, runs and results, all on your machine. Nothing is uploaded, and no account or licence server is needed to use the open-source build.

Dual-licensed MIT OR Apache-2.0. The design is in [spec.txt](spec.txt); open work is tracked in [TODO.md](TODO.md); the internal design is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## What it does today

- **Projects** in a directory: a Keystone database, runs, results, computational documents and content-addressed blobs (plots and data).
- **Documents** made of markdown and code cells. Code cells run in Rhai, a sandboxed embedded language with time, memory and operation limits. Each cell's outputs are stored with it.
- **Numerics** from the TPT stack: linear solves, normal sampling, ODE solves (four methods), and dimension-checked units.
- **Reproducibility**: a run uses a seed, every run is recorded, and a check reruns a document from scratch and compares the results.
- **Packages**: a project packs into one signed file (optionally encrypted) and unpacks elsewhere, with every file hash-checked.
- **Agents** (optional): capability-restricted, deny by default, every action logged.
- **Window**: an editor for documents, on AppFront's native canvas.

## Build and test

```
cargo build --workspace
cargo test --workspace
```

Requires the Rust toolchain in `rust-toolchain.toml`. The foundation crates come from GitHub and crates.io, so the first build needs network access. After that the program runs offline.

## Try it

```
cargo run -p research-app --example demo_project -- demo-project
cargo run -p research-app -- demo-project
```

The first command builds an example project and prints its outputs. The second opens it in the editor.

## Layout

| Crate | What it holds |
|-------|---------------|
| `research-core` | Typed ids, errors, the project manifest |
| `research-store` | Projects, runs, results, documents, blobs |
| `research-doc` | Document format and `.ipynb` import and export |
| `research-compute` | Cell execution, sandbox, numeric bindings |
| `research-package` | Signed, optionally encrypted project packages |
| `research-agents` | Capability-restricted agents and reproducibility checks |
| `research-license` | Offline licence key verification |
| `research-app` | Application model and editor window |

`vendor/tpt-sci-ode` is a vendored copy of an upstream crate. See its `VENDORED.md`.

## Licence

MIT OR Apache-2.0. Dependencies must be compatible with both; `deny.toml` enforces this. See [CONTRIBUTING.md](CONTRIBUTING.md) for the exceptions and how to add one.
