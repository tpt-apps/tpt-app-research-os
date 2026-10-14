# Security

## Reporting a vulnerability

Please report security problems privately. Use GitHub's private vulnerability reporting on this repository, not a public issue. Include what you found, how to reproduce it, and what you think the impact is.

## What the project protects

- **Package import.** A package's signature is checked first. Paths are checked for traversal, reserved Windows names and case collisions. Declared sizes bound what is read. Nothing is written until verification passes. See `docs/PACKAGE_FORMAT.md`.
- **Cell sandbox.** Cells run in Rhai with operation, time, string, collection and call-depth limits. No file, network or process functions are registered. See `docs/ARCHITECTURE.md`.
- **Agents.** An agent can act only through capabilities it was granted, and every attempt is logged.
- **Offline.** No telemetry and no network calls from the application. Keystone's server code is compiled into its library. A runtime check is still open, as recorded in `TODO.md` §10.

## Known limits

- Keystone (`tpt-keystone`) links networking code that this application does not call. Upstream should gate it behind a feature.
- Packages are signed, not secret. Use encryption for confidentiality.
