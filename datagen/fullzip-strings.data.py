"""Generate a FullZip page with variable-width string values.

The values are long enough (> 256 bytes) that the v2.2 writer chooses the
FullZip layout instead of mini-block. Used by the `show --detail` tests.

Run from the repository root:

    uv run --no-project --with pyarrow --with pylance python datagen/fullzip-strings.data.py
"""

from pathlib import Path
import shutil
import tempfile

import lance
import pyarrow as pa

ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = ROOT / "data"
RAW_DATA_PATH = DATA_DIR / "fullzip-strings.data.lance"
FIXTURE_PATH = ROOT / "tests" / "fixtures" / "fullzip_strings.lance"

def main() -> None:
    rows = [f"row-{index:02d}-" + "x" * 300 for index in range(8)]
    table = pa.table({"text": rows})
    DATA_DIR.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="ldp-datagen-") as tmp:
        dataset_path = Path(tmp) / "fullzip-strings.lance"
        dataset = lance.write_dataset(
            table,
            dataset_path,
            mode="create",
            data_storage_version="2.2",
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


if __name__ == "__main__":
    main()
