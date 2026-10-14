# Contributing

## Checks

Run these before you open a pull request. CI runs the same ones on Windows, macOS and Linux.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check bans licenses sources
```

## Rules that the checks enforce

- **Licences.** Every dependency must be compatible with MIT and Apache-2.0. Exceptions are listed in `deny.toml` with a reason. Add one only with a reason, and mark it for review.
- **Bans.** HTTP client crates and telemetry crates are banned, so the app cannot make network calls by accident. See `deny.toml`.
- **Git dependencies.** Each must be pinned to a `rev`, and its source URL must be on the `allow-git` list. Cargo resolves the lowercase `tpt-solutions` URLs, so use those.
- **Wildcards.** Every dependency needs a version requirement.

## Foundation crates

Foundation crates from `github.com/tpt-solutions/*` are pinned by `rev` in the workspace `Cargo.toml`. Two rules:

- Upstream repositories must parse with cargo. Cargo scans every manifest in a git repository, so placeholder manifests break a pin. When that happens, vendor the one crate under `vendor/` and record the source commit and every change in its `VENDORED.md`, as `tpt-sci-ode` does.
- Do not edit vendored sources in place. Re-vendor from upstream and apply the same changes.

## Decisions

Record a decision in `docs/ARCHITECTURE.md` with the reasons and the rejected options, as the cell language is recorded there. Update `TODO.md` at the same time.

## Commits

Work on a branch, not on `master`. Keep a commit to one change, and describe why in the message.
