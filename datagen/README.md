# Test data generators

Python scripts that regenerate the Lance fixtures used by `tests/`.

## `nested-empty-students.data.py`

Generates the nested-list example (`list<struct<name, list<int32>>>`) used by
`show --detail` tests.

Run from the repository root:

```bash
uv run --no-project --with pyarrow --with pylance python datagen/nested-empty-students.data.py
```

Outputs:

- `data/nested-empty-students.data.lance` - the single data file
- `tests/fixtures/nested_empty_students.lance` - the committed test fixture

The Lance dataset directory (manifest + data file) is created in a temporary
directory and discarded; only the bare data file is kept.

The script prints the Lance library version and the dataset storage version so
format changes are visible when regenerating the fixture.
