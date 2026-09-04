#!/usr/bin/env python3
"""Fail closed when an Android CycloneDX SBOM has unapproved licenses."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


ANDROID_COMPONENT = ("dev.envoix", "envoix-android")
ANDROID_CYCLONEDX_VERSION = "1.6"
ANDROID_ALLOWED_LICENSES = frozenset({"Apache-2.0", "BSD-3-Clause"})


def _required_string(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip() or value != value.strip():
        raise ValueError(f"{field} must be a non-empty canonical string")
    return value


def _component_license_ids(component: dict[str, Any], label: str) -> frozenset[str]:
    choices = component.get("licenses")
    if not isinstance(choices, list) or not choices:
        raise ValueError(f"{label} has no declared license")

    identifiers: set[str] = set()
    for index, choice in enumerate(choices):
        if not isinstance(choice, dict):
            raise ValueError(f"{label} license choice {index} must be an object")
        if "expression" in choice:
            raise ValueError(
                f"{label} uses an unaudited SPDX expression; add parser support before accepting it"
            )
        license_record = choice.get("license")
        if not isinstance(license_record, dict):
            raise ValueError(f"{label} license choice {index} has no license object")
        identifiers.add(
            _required_string(license_record.get("id"), f"{label} license choice {index} id")
        )
    return frozenset(identifiers)


def validate_android_sbom(path: Path) -> int:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read Android SBOM {path}: {error}") from error
    if not isinstance(document, dict):
        raise ValueError("Android SBOM root must be an object")
    if document.get("bomFormat") != "CycloneDX":
        raise ValueError("Android SBOM must use CycloneDX")
    if document.get("specVersion") != ANDROID_CYCLONEDX_VERSION:
        raise ValueError(
            f"Android SBOM must use CycloneDX {ANDROID_CYCLONEDX_VERSION}"
        )

    metadata = document.get("metadata")
    root = metadata.get("component") if isinstance(metadata, dict) else None
    if not isinstance(root, dict):
        raise ValueError("Android SBOM has no metadata component")
    actual_component = (
        _required_string(root.get("group"), "Android SBOM component group"),
        _required_string(root.get("name"), "Android SBOM component name"),
    )
    if actual_component != ANDROID_COMPONENT:
        raise ValueError(
            "Android SBOM component must be "
            f"{ANDROID_COMPONENT[0]}:{ANDROID_COMPONENT[1]}, got "
            f"{actual_component[0]}:{actual_component[1]}"
        )
    _required_string(root.get("version"), "Android SBOM component version")

    components = document.get("components")
    if not isinstance(components, list) or not components:
        raise ValueError("Android SBOM has no dependency components")

    violations: list[str] = []
    seen: set[tuple[str, str]] = set()
    for index, component in enumerate(components):
        if not isinstance(component, dict):
            raise ValueError(f"Android SBOM component {index} must be an object")
        name = _required_string(component.get("name"), f"component {index} name")
        version = _required_string(component.get("version"), f"component {index} version")
        identity = (name, version)
        if identity in seen:
            raise ValueError(f"Android SBOM repeats component {name}@{version}")
        seen.add(identity)

        license_ids = _component_license_ids(component, f"{name}@{version}")
        if license_ids.isdisjoint(ANDROID_ALLOWED_LICENSES):
            violations.append(f"{name}@{version} ({', '.join(sorted(license_ids))})")

    if violations:
        raise ValueError(
            "Android SBOM contains dependencies without an approved license choice: "
            + "; ".join(violations)
        )
    return len(components)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sbom", type=Path, help="CycloneDX JSON SBOM to validate")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        component_count = validate_android_sbom(args.sbom)
    except ValueError as error:
        raise SystemExit(f"Android SBOM license policy failed: {error}") from error
    allowed = ",".join(sorted(ANDROID_ALLOWED_LICENSES))
    print(
        "sbom_license_policy=passed "
        f"platform=android components={component_count} allowed={allowed} path={args.sbom}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
