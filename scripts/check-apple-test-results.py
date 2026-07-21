#!/usr/bin/env python3
"""Validate that an Apple test result bundle executed the expected suite."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


def non_negative_integer(value: str) -> int:
    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("must be non-negative")
    return parsed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path, help="Path to the .xcresult bundle")
    parser.add_argument("--label", required=True, help="Suite label used in reports")
    parser.add_argument("--minimum-total", required=True, type=non_negative_integer)
    parser.add_argument("--minimum-executed", required=True, type=non_negative_integer)
    parser.add_argument("--maximum-skipped", required=True, type=non_negative_integer)
    return parser.parse_args()


def count(summary: dict[str, object], key: str) -> int:
    value = summary.get(key)
    if not isinstance(value, int) or value < 0:
        raise ValueError(f"xcresult summary has invalid {key}: {value!r}")
    return value


def main() -> int:
    args = parse_args()
    if not args.result.is_dir():
        print(f"error: result bundle not found: {args.result}", file=sys.stderr)
        return 2

    completed = subprocess.run(
        [
            "xcrun",
            "xcresulttool",
            "get",
            "test-results",
            "summary",
            "--compact",
            "--path",
            str(args.result),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print(completed.stderr.rstrip(), file=sys.stderr)
        return completed.returncode

    try:
        summary = json.loads(completed.stdout)
        total = count(summary, "totalTestCount")
        passed = count(summary, "passedTests")
        failed = count(summary, "failedTests")
        skipped = count(summary, "skippedTests")
        expected_failures = count(summary, "expectedFailures")
    except (json.JSONDecodeError, ValueError) as error:
        print(f"error: could not parse xcresult summary: {error}", file=sys.stderr)
        return 2

    executed = passed + failed + expected_failures
    classified = executed + skipped
    report = (
        f"Apple test summary [{args.label}]: total={total} executed={executed} "
        f"passed={passed} failed={failed} expected-failures={expected_failures} "
        f"skipped={skipped}"
    )
    print(report)

    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a", encoding="utf-8") as output:
            output.write(f"- {report}\n")

    errors: list[str] = []
    if summary.get("result") != "Passed":
        errors.append(f"suite result is {summary.get('result')!r}, not 'Passed'")
    if failed != 0:
        errors.append(f"{failed} test(s) failed")
    if total < args.minimum_total:
        errors.append(f"total {total} is below required {args.minimum_total}")
    if executed < args.minimum_executed:
        errors.append(f"executed {executed} is below required {args.minimum_executed}")
    if skipped > args.maximum_skipped:
        errors.append(f"skipped {skipped} exceeds allowed {args.maximum_skipped}")
    if classified != total:
        errors.append(f"classified {classified} tests but summary reports total {total}")

    for error in errors:
        print(f"error: {args.label}: {error}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
