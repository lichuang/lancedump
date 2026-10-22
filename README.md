# lancedump

A read-only inspector for Lance data files.

`ldp` opens a single Lance data file (`data/*.lance`) and explains its physical
structure: the footer, the embedded schema, column metadata, pages, page layouts
(mini-block, full-zip, sparse, constant, blob), and the page-internal structures
behind them. It never modifies data and does not require a dataset manifest.

**Status:** design phase. This README defines the v1 command surface and the
order in which the commands are implemented. No commands are implemented yet.

---

## Scope

### Input

One local Lance **data file**:

```bash
ldp info path/to/data/xxxxxxxx.lance
```

Dataset directories, manifests, and object-store URIs are out of scope for v1.

### What a data file can answer by itself

Everything about the physical layout:

- format version and the five file regions (data, column metadata, CMO, GBO, footer)
- schema: field ids, names, logical types, nullability, metadata
- per-column page lists and buffer ranges
- per-page layout: `MiniBlock` / `FullZip` / `Sparse` / `Constant` / `Blob`, plus
  compression, rep/def layers, item counts, dictionary, repetition index depth
- page bytes: mini-block chunk metadata, chunk headers, rep/def/value ranges,
  repetition index and full-zip repetition index
- global buffers (schema is `GBO[0]`; the rest is opaque bytes with keys in
  schema metadata)

### What needs the dataset manifest (v1: shown as raw columns)

- **field id -> column index** mapping. The file stores the schema but not the
  mapping; it lives in the manifest's `DataFile.fields` / `column_indices`.
  Therefore `columns` labels columns by index, not by field name.
- deletion vectors, fragment ids, row addresses, stable row ids
- blob v2 sidecars and external base paths
- index coverage and dataset-level versioning

A future `--dataset <root>` option may load the manifest to bind columns to field
names and resolve the items above. It is not part of v1.

---

## Command surface

All commands take exactly one file. `--format text|json` is global; `text` is the
default and `json` is stable enough for scripting.

The table is ordered by implementation dependency: each command only relies on
what earlier commands already parse.

| Order | Command | Adds | Answers |
|---|---|---|---|
| M0 | `ldp hexdump <FILE>` | byte-range reads | "what are these raw bytes?" |
| M1 | `ldp info <FILE>` | footer, GBO/CMO, schema + columns load | "what is this file?" |
| M2 | `ldp schema <FILE>` | schema tree rendering | "what logical fields does it contain?" |
| M3 | `ldp columns <FILE>` | column summaries | "how many pages/bytes per column?" |
| M4 | `ldp pages <FILE>` | `PageLayout` decoding | "where are the pages and how are they laid out?" |
| M5 | `ldp page <FILE>` | page-byte I/O, chunk tables, rep index | "what is inside this one page?" |
| M6 | `ldp chunks <FILE>` | mini-block focus (subset of M5) | "show me the chunk table" |
| M7 | `ldp check <FILE>` | validation over M1-M6 | "is this file structurally sane?" |

Dependency graph:

```text
hexdump                         (independent primitive)

info                            container layer
 ├── schema                     view of GBO[0]
 ├── columns                    view of CMO
 ├── pages                      + decode PageLayout (metadata only)
 │    └── page                  + read page bytes, chunk table, rep index
 │         └── chunks           mini-block subset of page
 └── check                      validation on top of all of the above
```

---

## Implementation order

Each milestone should be finished (implemented, exercised against real files, and
documented) before starting the next one.

### M0 - `hexdump`

No parser dependencies. Establishes `read_at(offset, len)` and the hex renderer.

Done when:

- `ldp hexdump f.lance --offset 0 --length 64` prints an offset / hex / ASCII view
- out-of-range reads fail with a clear error, not a panic

### M1 - `info`

The foundational command. It must load the container without touching any page
bytes or `PageLayout`.

Done when:

- parses the 40-byte footer (magic, version, three offsets, two counts)
- validates region order and CMO/GBO table sizes
- decodes `GBO[0]` as `FileDescriptor` (schema + row count)
- decodes every column metadata record (page count, buffer ranges)
- prints path, size, version, rows, region table, global buffer table, column
  summary
- legacy v1 files print version + regions with an explicit "not decoded" warning
- `--format json` emits the same fields

### M2 - `schema`

Pure rendering of the schema already loaded by M1.

Done when:

- prints a tree using `parent_id`, including field id, name, logical type,
  nullability, and field metadata
- prints schema-level metadata
- malformed parent links degrade to a flat listing instead of failing

### M3 - `columns`

Done when:

- one row per column: page count, summed page rows, column-buffer count, bytes,
  column encoding kind, layout kinds present
- `--column N` filters and reports a useful error for a missing index
- no page bytes are read (still metadata-only)

### M4 - `pages`

First milestone that decodes `Page.encoding` (`Any` -> `PageLayout`).

Done when:

- one row per page: column, page, position, bytes, rows, priority, buffer count,
  layout kind and a short parameter summary
- `--column N` filters
- unknown/undecodable encodings become warnings, not hard failures

### M5 - `page`

Deepest per-page view.

Done when:

- decodes the full `PageLayout`, including compression names and rep/def layers
- lists page buffers with roles (chunk metadata / chunk data / dictionary /
  repetition index / zipped values)
- parses the mini-block chunk table: 2-byte (v2.1) or 4-byte (v2.2+) metadata
  entries, chunk offsets/sizes, value counts, chunk header, rep/def/value ranges
- parses the mini-block repetition index (`depth + 1` u64 per chunk) and the
  full-zip repetition index (byte-packed width + sentinel)
- `--hex` adds a bounded hex dump per buffer; `--hex-limit` controls the bound
- `--raw` prints the decoded protobuf in debug form
- inconsistencies are reported as warnings; the command still exits 0

### M6 - `chunks`

The mini-block chunk table as its own output, for comparing against the format
spec.

Done when:

- works for mini-block pages and rejects other layouts with a clear message
- prints metadata entry width, chunk count, item/value totals, and per-chunk
  offset/bytes/values/log2/levels/rep/def/header/value-buffer sizes
- prints repetition-index words with their meaning (`rows ending`, `leftover items`)

### M7 - `check`

Validation over everything above.

Done when:

- checks magic/version, region order, CMO/GBO sizes, metadata bounds, global
  buffer bounds, page-buffer 64-byte alignment and bounds
- decodes every page layout and every mini-block chunk table; reports warnings
- exits non-zero when any check fails (unlike the informational commands)
- `--format json` lists each check with `ok` / `warn` / `error`

---

## Command reference

### `ldp info <FILE>`

File-level summary. Never reads page bytes.

```text
              path: data/xxxx.lance
              size: 53.4 KiB
            format: v2.2 (2.2)
              rows: 66049
           columns: 2
    global buffers: 1
            schema: 3 field(s), 0 metadata key(s)

regions
name             start    end      size
...
```

### `ldp schema <FILE>`

```text
fields: 3

#0   mime                         string               nullable
#1   captions                     list                 nullable
  #2   item                         uint32               nullable
```

### `ldp columns <FILE> [--column N]`

```text
column  pages   rows  col bufs     bytes  encoding    layouts
------  -----  -----  --------  --------  ----------  ----------
     0      1  66049         0      30 B  none        constant
     1      1  66049         0  52.7 KiB  none        mini-block
```

### `ldp pages <FILE> [--column N]`

```text
column  page  position     bytes   rows  priority  bufs  layout
------  ----  --------  --------  -----  --------  ----  -----------------------------
     1     0        64  52.7 KiB  66049         0     3  mini-block (u32 chunk meta, 1 value buffer(s))
```

### `ldp page <FILE> --column N --page K [--hex] [--hex-limit BYTES] [--raw]`

Shows one page end to end: layout parameters, page buffers and their roles, the
mini-block chunk table, and the repetition index.

```bash
ldp page data/xxxx.lance --column 1 --page 0
ldp page data/xxxx.lance --column 1 --page 0 --hex --hex-limit 256
```

### `ldp chunks <FILE> --column N --page K`

```text
chunk  offset  bytes  values  log2  levels  rep    def  header  value bufs  rows ending  leftover
-----  ------  -----  ------  ----  ------  ---  -----  ------  ----------  -----------  --------
    0       0   1760    1024    10    1024  200    256      16       1284           32         0
```

### `ldp hexdump <FILE> --offset O --length L [--width BYTES]`

```bash
ldp hexdump data/xxxx.lance --offset 0 --length 256 --width 16
```

### `ldp check <FILE>`

```text
[ ok ] footer magic: file ends with "LANC"
[ ok ] format version: v2.2
[ ok ] region order: ...
[ ok ] mini-block chunk tables: all mini-block chunk tables parsed

11 check(s): 0 error(s), 0 warning(s)
```

---

## Output

- `--format text` (default): UTF-8 tables, aligned for terminal viewing.
- `--format json`: pretty-printed, one document per invocation; field names are
  part of the CLI contract and should stay stable.
- Informational commands (`info`, `schema`, `columns`, `pages`, `page`, `chunks`)
  exit `0` even when they report warnings. `check` exits non-zero on errors.
- All offsets and sizes are decimal bytes in text and JSON; hex dumps use
  lowercase hex with an ASCII gutter.

---

## Non-goals (v1)

- writing, repairing, or rewriting files
- reading dataset manifests (`_versions/*.manifest`), fragments, deletions, indexes
- decoding Arrow values (`ldp rows`) - layout first, values later
- legacy v1 file bodies (footer/regions only)
- object storage (S3/GCS/Azure) and remote URIs
- blob v2 sidecar resolution

## Open questions

1. Dependency strategy: reuse the `lance-file` / `lance-encoding` crates, or parse
   the container and protobufs independently? (The command surface is the same;
   this affects correctness-vs-control.)
2. Where to vendor the `.proto` files and how to handle `protoc` in CI.
3. Whether `page` should default to the deep view or require `--deep`.
4. Whether `columns`/`pages` should stay separate or collapse into `ldp list`.
