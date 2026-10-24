"""Generate the nested-list example from `miniblock-encoding.md` chapter 6.

Schema
------
    students: list<struct<
        name:   string,          -- non-null
        scores: list<int32>      -- non-null list, nullable item
    >>                           -- non-null list, may be empty

Data
----
    row 0: [ {name: "a", scores: [10, 20]}, {name: "b", scores: []} ]
    row 1: []
    row 2: [ {name: "c", scores: [30, null]} ]
    row 3: [ {name: "d", scores: [40]} ]

The dataset is written with the Lance 2.1 structural format so the mini-block
chunk header uses 16-bit buffer sizes, matching the byte layout in the doc.
"""

from pathlib import Path
import shutil
import tempfile

import lance
import pyarrow as pa

ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = ROOT / "data"
RAW_DATA_PATH = DATA_DIR / "nested-empty-students.data.lance"
FIXTURE_PATH = ROOT / "tests" / "fixtures" / "nested_empty_students.lance"


def build_table() -> pa.Table:
    scores_type = pa.list_(pa.field("item", pa.int32(), nullable=True))
    student_type = pa.struct(
        [
            pa.field("name", pa.string(), nullable=False),
            pa.field("scores", scores_type, nullable=False),
        ]
    )
    students_type = pa.list_(pa.field("item", student_type, nullable=False))
    schema = pa.schema([pa.field("students", students_type, nullable=False)])

    rows = [
        [
            {"name": "Hi", "scores": [10, 20]},
            {"name": "a", "scores": []},
        ],
        [],
        [
            {"name": "Hello", "scores": [30, None]},
        ],
        [
            {"name": "test", "scores": [40]},
        ],
    ]

    return pa.Table.from_arrays(
        [pa.array(rows, type=students_type)],
        schema=schema,
    )


def main() -> None:
    table = build_table()
    DATA_DIR.mkdir(parents=True, exist_ok=True)

    # The dataset directory (manifest + data file) is only an intermediate; the
    # dumper consumes the bare data file.
    with tempfile.TemporaryDirectory(prefix="ldp-datagen-") as tmp:
        dataset_path = Path(tmp) / "nested-empty-students.lance"
        dataset = lance.write_dataset(
            table,
            dataset_path,
            mode="create",
            data_storage_version="2.1",
        )

        data_files = sorted((dataset_path / "data").glob("*.lance"))
        if len(data_files) != 1:
            raise RuntimeError(f"expected exactly one data file, found {data_files}")
        shutil.copyfile(data_files[0], RAW_DATA_PATH)

    FIXTURE_PATH.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(RAW_DATA_PATH, FIXTURE_PATH)

    print(f"data    : {RAW_DATA_PATH}  ({RAW_DATA_PATH.stat().st_size} bytes)")
    print(f"fixture : {FIXTURE_PATH}")
    print(f"lance   : {lance.__version__}")
    print(f"version : {dataset.data_storage_version}")
    print()
    print(table.to_pylist())


if __name__ == "__main__":
    main()
