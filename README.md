# lancedump

A read-only inspector for Lance data files.

`ldp` opens a single Lance data file (`data/*.lance`) and explains its physical
structure: the footer, the embedded schema, column metadata, pages, page layouts
(mini-block, full-zip, sparse, constant, blob), and the page-internal structures
behind them.

`ldp` never modifies data and does not require a dataset manifest.

> Development status and implementation phases: [docs/phase.md](docs/phase.md).

---

## Install

```bash
cargo build --release
# binary: target/release/ldp

# or install into ~/.cargo/bin
cargo install --path .
```

Running from the repository:

```bash
cargo run -- info path/to/data/xxxxxxxx.lance
```

---

## Input

One local Lance **data file**:

```bash
ldp info path/to/data/xxxxxxxx.lance
```

Dataset directories, manifests, and object-store URIs are not accepted in v1.

A Lance data file describes its physical layout by itself. The following items
are **not** available without the dataset manifest, and `ldp` reports them as
raw columns instead:

- field id -> column index mapping (manifest `DataFile.fields` / `column_indices`)
- deletion vectors, fragment ids, row addresses, stable row ids
- blob v2 sidecars and external base paths
- index coverage and dataset-level versioning

---

## Commands

All commands take exactly one file. `--format text|json` is global:

- `text` (default): aligned UTF-8 tables for terminal viewing
- `json`: pretty-printed JSON, one document per invocation
- Text output is colorized automatically when stdout is a terminal. Set
  `NO_COLOR` to disable it, or `CLICOLOR_FORCE=1` to force color through a pipe.

### `ldp info <FILE>`

File-level summary: format version, file regions, global buffers, and one row per
column. Never reads page bytes.

```bash
ldp info data/xxxx.lance
ldp info data/xxxx.lance --format json
```

```text
Summary
              Path: data/xxxx.lance
              Size: 53.4 KiB
            Format: v2.2 (2.2)
              Rows: 66049
           Columns: 2
    Global buffers: 1
            Schema: 3 field(s)

Footer
  layout                      v2
  column_meta_start           54304
  column_meta_offsets_start   54557
  global_buff_offsets_start   54589
  num_global_buffers          1
  num_columns                 2

Regions
name                          start    end      size
----------------------------  -----  -----  --------
data                              0  54304  53.0 KiB
column metadata               54304  54557     253 B
column metadata offset table  54557  54589      32 B
global buffer offset table    54589  54605      16 B
footer                        54605  54645      40 B

Column Metadata Offset Table
column  position  meta size  pages   rows  buffers  buf bytes
------  --------  ---------  -----  -----  -------  ---------
     0     54304       99 B      1  66049        1       30 B
     1     54403      154 B      1  66049        3   52.7 KiB

Global Buffer Offset Table
index  position  size  description
-----  --------  ----  -------------------------------------------------
    0     54208  96 B  schema (FileDescriptor: 3 field(s), 66049 row(s))
```

### `ldp schema <FILE>`

The schema embedded in global buffer 0, rendered as a tree.

```bash
ldp schema data/xxxx.lance
```

```text
Fields: 3

id  field     type    nullable  path
--  --------  ------  --------  -------------
 0  mime      string  yes       mime
 1  captions  list    yes       captions
 2  └─ item   uint32  yes       captions.item
```

### `ldp columns <FILE> [--column N]`

One row per column.

```bash
ldp columns data/xxxx.lance
ldp columns data/xxxx.lance --column 1
```

```text
column  pages   rows  col bufs     bytes  encoding    layouts
------  -----  -----  --------  --------  ----------  ----------
     0      1  66049         0      30 B  none        constant
     1      1  66049         0  52.7 KiB  none        mini-block
```

| Option | Description |
|---|---|
| `--column N` | Restrict output to column `N` |

### `ldp pages <FILE> [--column N]`

One row per page: buffer range, size, row count, priority, buffer count, and layout.
`#` is the page's sequential position in the file's physical order (rows are
sorted by `start`), and stays stable when `--column` is used.
`page (in column)` is the page index within its column (the value `show --page`
expects), so different columns both start at page 0. `start`/`end` are the
smallest buffer offset and one past the largest buffer end; `end` is exclusive
and excludes 64-byte alignment padding.

Layout kinds are colorized when stdout is a terminal: mini-block (green),
full-zip (yellow), sparse (magenta), constant (blue), blob (cyan), legacy
v2.0 (dim).

```bash
ldp pages data/xxxx.lance
ldp pages data/xxxx.lance --column 1
```

```text
column  page (in column)  position     bytes   rows  priority  bufs  layout
------  ---------------  --------  --------  -----  --------  ----  -----------------------------
     1     0        64  52.7 KiB  66049         0     3  mini-block (u32 chunk meta, 1 value buffer(s))
```

### `ldp show <FILE> --column N --page K`

Full anatomy of one page: layout parameters, page buffers and their roles, the
mini-block chunk table, and the repetition index.

```bash
ldp show data/xxxx.lance --column 1 --page 0
ldp show data/xxxx.lance --column 1 --page 0 --hex --hex-limit 256
ldp show data/xxxx.lance --column 1 --page 0 --raw
```

| Option | Description |
|---|---|
| `--column N` | Column index (required) |
| `--page K` | Page index within the column (required) |
| `--hex` | Include a hex dump of each page buffer |
| `--hex-limit BYTES` | Maximum bytes dumped per buffer (default: 1024) |
| `--raw` | Print the decoded protobuf layout in debug form |

### `ldp chunks <FILE> --column N --page K`

The mini-block chunk table of one page, for comparing against the format spec.

```bash
ldp chunks data/xxxx.lance --column 1 --page 0
```

```text
chunk  offset  bytes  values  log2  levels  rep    def  header  value bufs  rows ending  leftover
-----  ------  -----  ------  ----  ------  ---  -----  ------  ----------  -----------  --------
    0       0   1760    1024    10    1024  200    256      16       1284           32         0
```

### `ldp hexdump <FILE>`

Hex dump an arbitrary byte range.

```bash
ldp hexdump data/xxxx.lance --offset 0 --length 256 --width 16
```

| Option | Description |
|---|---|
| `--offset BYTES` | Start offset (required) |
| `--length BYTES` | Number of bytes to dump (required) |
| `--width BYTES` | Bytes per line (default: 16) |

### `ldp check <FILE>`

Validate the container and decode every page layout and mini-block chunk table.

```bash
ldp check data/xxxx.lance
ldp check data/xxxx.lance --format json
```

```text
[ ok ] footer magic: file ends with "LANC"
[ ok ] format version: v2.2
[ ok ] region order: ...
[ ok ] mini-block chunk tables: all mini-block chunk tables parsed

11 check(s): 0 error(s), 0 warning(s)
```

Exit codes:

- `0` - success. Informational commands (`info`, `schema`, `columns`, `pages`,
  `show`, `chunks`) also exit `0` when they emit warnings.
- non-zero - error. `check` exits non-zero when any check fails.

---

## Limitations

- Read-only. `ldp` never writes, repairs, or rewrites files.
- Local filesystem only. Object storage and remote URIs are not supported.
- No dataset manifest support, so columns are identified by index rather than by
  field name, and deletions / row ids / indexes are not shown.
- Legacy v1 files are rejected. Only v2.0-v2.3 are supported.
- Page layouts are described, not decoded into Arrow values.
- Blob v2 payloads stored in sidecar files are not resolved.

---

## License

Apache-2.0
