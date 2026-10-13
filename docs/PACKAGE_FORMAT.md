# Project package format

Status: format version 1, implemented in `crates/research-package`.
A package is one file that carries a whole research project between machines.

## Contents

A package is a zip archive with these entries:

| Entry | Purpose |
|-------|---------|
| `package.json` | The package manifest (below). Signed. |
| `package.sig` | Ed25519 signature over the exact bytes of `package.json`. |
| every other file | The project directory, as it was packed: `manifest.json`, the Keystone database under `local/` and `objects/`, documents, data files. |

Paths use `/` as the separator. The project's `.lock` file is never packed.

## Package manifest

```json
{
  "format_version": 1,
  "project_id": "study-1",
  "created": "2026-10-14T00:00:00Z",
  "signer": "<64 hex digits: Ed25519 public key>",
  "entries": [
    { "path": "manifest.json", "sha256": "<64 hex digits>", "size": 212 },
    { "path": "local/...", "sha256": "...", "size": 4096 }
  ]
}
```

- `entries` lists every file in the archive except `package.json` and `package.sig`, sorted by path.
- `project_id` must match the `id` in the packed `manifest.json`.
- `signer` is recorded so a receiver can see who signed. It is not trusted by itself: the receiver compares it with a key it already knows.

## Packing

`pack_project(dir, created, signing_key)`:

1. reads and validates the project's `manifest.json`;
2. takes the project lock. If another handle has the project open, packing fails, since its files may be changing;
3. refuses symbolic links anywhere in the project;
4. checks every path (below) and packs the files.

## Import

`verify(bytes, verifier_key)` rejects the package if any of these fail, in this order. Nothing is written to disk until all of them pass:

1. the archive is a readable zip;
2. `package.sig` verifies against the supplied key;
3. `format_version` is supported;
4. every listed path is safe, is not `package.json` or `package.sig`, and does not collide with another listed path, ignoring case;
5. the listed sizes sum to no more than 2 GiB;
6. the archive holds no entry the manifest does not list, and no name appears twice;
7. each file is read with at most its declared size plus one byte, then its length and SHA-256 must match;
8. the packed `manifest.json` parses, and its `id` equals `project_id`.

`unpack_project(bytes, verifier_key, dest)` runs `verify`, writes into `dest.partial`, and renames it to `dest`. It refuses if `dest` or `dest.partial` already exists, so an import never merges into or overwrites a project.

## Path rules

A path is accepted only if it:

- is 512 characters or fewer, and not empty;
- is relative: no leading `/`;
- has no `\`, `:`, `*`, `?`, `"`, `<`, `>`, `|`, or control characters;
- has no empty, `.` or `..` component;
- has no component ending in `.` or space (Windows strips these);
- has no component whose stem is a Windows device name (`CON`, `NUL`, `COM1`, and so on).

These rules apply on export as well as import, so a package made on Linux still unpacks safely on Windows.

## Signing keys

`generate_signing_key()` draws 32 bytes from the operating system's random source. Storing and distributing keys is the caller's job; the format does not define a key file.

## Encryption

`encrypt(package, passphrase)` wraps a whole package:

```
"RSPKGENC" | 16-byte salt | 12-byte nonce | ChaCha20-Poly1305 ciphertext
```

The key is derived from the passphrase with Argon2id (crate defaults). `decrypt` passes plain packages through unchanged, so a caller can always call it first. Encryption is optional and separate from signing. A signed package is readable by anyone; encrypted packages are not.

## Versioning

`format_version` in the package manifest covers the container. The project inside has its own `format_version` in `manifest.json`, which is checked when the project is opened. A reader refuses any version it does not know.

## Known limits

- Packing reads the whole project into memory. Large projects will need streaming, which is not yet implemented.
- Unpacking a package does not check the file system's free space first.
- Zip timestamps are not preserved.
