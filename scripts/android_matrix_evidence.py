#!/usr/bin/env python3
"""Read endpoint evidence from instrumentation without a debuggable target APK."""
from __future__ import annotations

import argparse
import base64
import binascii
import json
from pathlib import Path

PREFIX = "INSTRUMENTATION_STATUS: envoixMatrixEvidence="
MAX_ENCODED_BYTES = 512 * 1024


def extract_evidence(log: Path) -> dict:
    results = []
    with log.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            if not line.startswith(PREFIX):
                continue
            encoded = line[len(PREFIX):].strip()
            if len(encoded) > MAX_ENCODED_BYTES:
                raise ValueError("Android endpoint evidence exceeds the size limit")
            try:
                result = json.loads(base64.b64decode(encoded, validate=True))
            except (binascii.Error, UnicodeError, json.JSONDecodeError) as error:
                raise ValueError("Android endpoint evidence is malformed") from error
            if not isinstance(result, dict):
                raise ValueError("Android endpoint evidence must be a JSON object")
            results.append(result)
    if len(results) != 1:
        raise ValueError("Expected exactly one Android endpoint evidence record")
    return results[0]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        result = extract_evidence(args.log)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Android evidence extraction failed: {error}\n")
    args.output.write_text(json.dumps(result) + "\n", encoding="utf-8")
    args.output.chmod(0o600)


if __name__ == "__main__":
    main()
