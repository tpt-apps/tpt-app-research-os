# Computational document format

Status: format version 1, implemented in `crates/research-doc`.
A document is the notebook-style unit of a research project: narrative text, code cells and their outputs.

## Goals

- **Diff-friendly.** Serializing the same document twice gives the same bytes. Keys are in a fixed order, and maps are sorted.
- **Stable cell ids.** Each cell has an id that survives edits, reordering and round-trips, so results and comments can point at a cell.
- **Interoperable.** Jupyter `.ipynb` (nbformat 4) can be imported and exported. Anything that cannot be represented is reported, not silently dropped.

## File layout

A document is a single UTF-8 JSON file, pretty-printed with two-space indentation and a trailing newline. The extension is `.rdoc.json`.

```json
{
  "format_version": 1,
  "id": "doc-spring-study",
  "title": "Damped oscillator",
  "metadata": {
    "author": "A. Researcher"
  },
  "cells": [
    {
      "id": "intro",
      "type": "markdown",
      "source": "We solve $m\\ddot{x} + c\\dot{x} + kx = 0$."
    },
    {
      "id": "solve",
      "type": "code",
      "language": "rhai",
      "source": "let k = 2.5;",
      "outputs": [
        { "type": "text", "text": "k = 2.5" }
      ]
    }
  ]
}
```

## Fields

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `format_version` | integer | yes | Currently `1`. A reader refuses a higher version. |
| `id` | string | yes | A `DocId`: ASCII letters, digits, `-`, `_`, at most 128 characters. |
| `title` | string | yes | May be empty. |
| `metadata` | object of string to string | no | Omitted when empty. Sorted by key. |
| `cells` | array | yes | Order is the reading order. |

### Cells

Every cell has:

- `id`: a `CellId`, same rules as `DocId`. Unique within the document.
- `type`: `"markdown"` or `"code"`.
- `source`: the text of the cell.

Code cells also have:

- `language`: the language the cell is written in. The compute layer decides which languages it can run.
- `outputs`: an array, possibly empty. Each output has a `type`:
  - `"text"`: `text`, a string. Standard output or a plain-text result.
  - `"error"`: `name` and `message`, strings.
  - `"table"`: `columns`, an array of strings, and `rows`, an array of arrays of strings. Each row has one entry per column.

Outputs are stored with the cell so a document can be read without re-running it. Whether outputs are current is the compute layer's concern, not the format's.

## Validation

A document is rejected when:

- `format_version` is greater than the supported version;
- `id` or any cell `id` is not a valid id, or two cells share an id;
- a table output's rows do not match its column count.

Blank titles and empty sources are allowed.

## Jupyter (`.ipynb`) mapping

Import reads nbformat 4 notebooks. Export writes nbformat 4.5, so cells carry ids.

| Notebook | Document |
|----------|----------|
| cell `id` (nbformat 4.5) | cell `id`, if valid and unique. Otherwise `imported-<index>`. |
| `markdown` cell | `markdown` cell |
| `code` cell | `code` cell. `language` is the cell's `research_os.language` metadata if present, otherwise the kernel language. |
| `stream` output | `text` output |
| `execute_result` / `display_data` with `text/plain` | `text` output |
| `error` output | `error` output |
| anything else (images, HTML, widgets) | dropped, with a warning |

Document-level `title` and `metadata`, and each code cell's `language`, are stored under `metadata.research_os` so they survive a round-trip.

Import takes the list of hosted languages. A code cell in any other language is still imported, with a warning, so the user can see what will not run.

Things that do not round-trip:

- execution counts are discarded;
- `table` outputs export as `text/plain` and come back as `text`;
- rich outputs are dropped on import, as described above.

## Versioning

Adding optional fields is not a version change. Anything that changes the meaning of an existing field, or makes an older reader misread a document, bumps `format_version`. Migrations between versions belong in `research-doc`, and are tested against stored sample files.
