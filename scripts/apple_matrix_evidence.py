#!/usr/bin/env python3
"""Extract one bounded matrix endpoint attachment exported from an xcresult."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any, Sequence


MAX_EXPORTED_FILES = 4096
MAX_ATTACHMENT_BYTES = 4 * 1024 * 1024


class EvidenceExtractionError(ValueError):
    """The exported attachment set did not contain one unambiguous result."""


def _matches_identity(value: object, expected: dict[str, Any]) -> bool:
    return isinstance(value, dict) and all(
        value.get(field) == expected_value
        for field, expected_value in expected.items()
    )


def find_endpoint_attachment(
    export_directory: Path,
    *,
    run_id: str,
    case_id: str,
    repetition: int,
    role: str,
    platform: str,
) -> Path:
    """Return the unique small JSON attachment with the expected identity."""

    if not export_directory.is_dir():
        raise EvidenceExtractionError("Apple attachment export directory is missing")

    expected = {
        "schema_version": 1,
        "run_id": run_id,
        "case_id": case_id,
        "repetition": repetition,
        "role": role,
        "platform": platform,
    }
    candidates: list[Path] = []
    file_count = 0
    for root, directories, files in os.walk(export_directory, followlinks=False):
        directories[:] = [
            name for name in directories if not (Path(root) / name).is_symlink()
        ]
        for name in files:
            file_count += 1
            if file_count > MAX_EXPORTED_FILES:
                raise EvidenceExtractionError(
                    f"Apple attachment export exceeds {MAX_EXPORTED_FILES} files"
                )
            path = Path(root) / name
            if path.is_symlink():
                continue
            try:
                size = path.stat().st_size
            except OSError:
                continue
            if size <= 0 or size > MAX_ATTACHMENT_BYTES:
                continue
            try:
                value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError):
                continue
            if _matches_identity(value, expected):
                candidates.append(path)

    if not candidates:
        raise EvidenceExtractionError(
            "Apple result bundle has no endpoint attachment with the expected identity"
        )
    if len(candidates) != 1:
        raise EvidenceExtractionError(
            "Apple result bundle has multiple endpoint attachments with the expected identity"
        )
    return candidates[0]


def extract_endpoint_attachment(
    export_directory: Path,
    output: Path,
    **expected: Any,
) -> None:
    source = find_endpoint_attachment(export_directory, **expected)
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, output)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("export_directory", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--case", required=True, dest="case_id")
    parser.add_argument("--repetition", required=True, type=int)
    parser.add_argument("--role", required=True, choices=("sender", "receiver"))
    parser.add_argument("--platform", required=True, choices=("ios", "macos"))
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        extract_endpoint_attachment(
            args.export_directory,
            args.output,
            run_id=args.run_id,
            case_id=args.case_id,
            repetition=args.repetition,
            role=args.role,
            platform=args.platform,
        )
    except EvidenceExtractionError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
