#!/usr/bin/env python3
"""Validate immutable CI dependencies and cross-platform release versions."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


FULL_COMMIT = re.compile(r"^[0-9a-f]{40}$")
ACTION_USE = re.compile(r"^\s*-?\s*uses:\s*([^\s#]+)", re.MULTILINE)
ANDROID_VERSION = re.compile(r'^\s*versionName\s*=\s*"([^"]+)"', re.MULTILINE)
ANDROID_BUILD = re.compile(r"^\s*versionCode\s*=\s*([0-9]+)", re.MULTILINE)
APPLE_VERSION = re.compile(r'^\s*MARKETING_VERSION:\s*"([^"]+)"', re.MULTILINE)
APPLE_BUILD = re.compile(r'^\s*CURRENT_PROJECT_VERSION:\s*"([0-9]+)"', re.MULTILINE)
MACOS_ARCHIVE_VERSION = re.compile(r'Envoix-([0-9]+\.[0-9]+\.[0-9]+)-macos-notarized\.zip')


@dataclass(frozen=True)
class ReleaseContract:
    version: str
    build_number: int
    action_count: int


def one_value(label: str, values: list[str]) -> str:
    distinct = sorted(set(values))
    if len(distinct) != 1:
        rendered = ", ".join(distinct) if distinct else "none"
        raise ValueError(f"{label} must have one value; found {rendered}")
    return distinct[0]


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        raise ValueError(f"cannot read {path}: {error}") from error


def validate_action_refs(root: Path) -> int:
    workflow_directory = root / ".github" / "workflows"
    workflows = sorted(workflow_directory.glob("*.yml")) + sorted(
        workflow_directory.glob("*.yaml")
    )
    if not workflows:
        raise ValueError("no GitHub Actions workflows found")

    action_count = 0
    for workflow in workflows:
        for reference in ACTION_USE.findall(read_text(workflow)):
            if reference.startswith("./") or reference.startswith("docker://"):
                continue
            if "@" not in reference:
                raise ValueError(f"{workflow}: action reference has no revision: {reference}")
            _, revision = reference.rsplit("@", 1)
            if not FULL_COMMIT.fullmatch(revision):
                raise ValueError(
                    f"{workflow}: action revision must be a 40-character commit SHA: {reference}"
                )
            action_count += 1
    if action_count == 0:
        raise ValueError("no external GitHub Actions references found")
    return action_count


def validate_versions(root: Path, tag: str | None = None) -> tuple[str, int]:
    cargo_path = root / "Cargo.toml"
    try:
        cargo = tomllib.loads(read_text(cargo_path))
        cargo_version = cargo["workspace"]["package"]["version"]
    except (KeyError, tomllib.TOMLDecodeError) as error:
        raise ValueError(f"cannot read workspace version from {cargo_path}: {error}") from error
    if not isinstance(cargo_version, str):
        raise ValueError("workspace version must be a string")

    android = read_text(root / "android" / "app" / "build.gradle.kts")
    apple = read_text(root / "apps" / "envoix-apple" / "project.yml")
    apple_release = read_text(root / "scripts" / "apple-dev.sh")

    android_version = one_value("Android versionName", ANDROID_VERSION.findall(android))
    apple_version = one_value("Apple MARKETING_VERSION", APPLE_VERSION.findall(apple))
    archive_version = one_value(
        "macOS release archive version", MACOS_ARCHIVE_VERSION.findall(apple_release)
    )
    versions = {cargo_version, android_version, apple_version, archive_version}
    if len(versions) != 1:
        raise ValueError(f"release versions disagree: {', '.join(sorted(versions))}")

    android_build = int(one_value("Android versionCode", ANDROID_BUILD.findall(android)))
    apple_build = int(one_value("Apple CURRENT_PROJECT_VERSION", APPLE_BUILD.findall(apple)))
    if android_build != apple_build:
        raise ValueError(
            f"release build numbers disagree: Android {android_build}, Apple {apple_build}"
        )
    if android_build <= 0:
        raise ValueError("release build number must be positive")

    expected_tag = f"v{cargo_version}"
    if tag is not None and tag != expected_tag:
        raise ValueError(f"release tag must be {expected_tag}, found {tag}")
    return cargo_version, android_build


def validate(root: Path, tag: str | None = None) -> ReleaseContract:
    version, build_number = validate_versions(root, tag)
    return ReleaseContract(
        version=version,
        build_number=build_number,
        action_count=validate_action_refs(root),
    )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--tag", help="Require the exact v<workspace-version> release tag")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        contract = validate(args.root.resolve(), args.tag)
    except ValueError as error:
        print(f"release contract error: {error}", file=sys.stderr)
        return 1
    print(
        "release contract ok: "
        f"version={contract.version} build={contract.build_number} "
        f"pinned_actions={contract.action_count}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
