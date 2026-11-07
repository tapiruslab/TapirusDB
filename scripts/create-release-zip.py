"""Create a release ZIP with deterministic POSIX archive paths."""

import argparse
from pathlib import Path, PurePosixPath
from zipfile import ZIP_DEFLATED, ZipFile


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    source = args.source.resolve()
    output = args.output.resolve()
    if not source.is_dir():
        raise SystemExit(f"Missing release directory: {source}")
    output.parent.mkdir(parents=True, exist_ok=True)

    with ZipFile(output, "w", compression=ZIP_DEFLATED) as archive:
        for path in sorted(source.rglob("*")):
            if not path.is_file():
                continue
            relative = path.relative_to(source)
            archive_name = PurePosixPath(*relative.parts).as_posix()
            archive.write(path, archive_name)


if __name__ == "__main__":
    main()
