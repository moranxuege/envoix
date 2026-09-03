#!/usr/bin/env python3
"""Validate and describe a signed Envoix Android release bundle."""

from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from collections import Counter
from pathlib import Path

from release_bundle import (
    Artifact,
    GIT_COMMIT,
    MAX_SBOM_BYTES,
    REPOSITORY,
    VERSION,
    sbom_serial,
    sha256,
    validate_regular_file,
)


APPLICATION_ID = "dev.envoix.app"
ANDROID_PACKAGES = ("envoix-android.apk", "envoix-android.aab")
ANDROID_SBOMS = {
    "envoix-android.cdx.json": ("envoix-android", "1.6"),
    "envoix-android-rust.cdx.json": ("envoix-ffi", "1.5"),
}
GENERATED_FILES = ("android-release-manifest.json", "SHA256SUMS.android")
CERTIFICATE_SHA256 = re.compile(r"^[0-9a-f]{64}$")
ANDROID_ABIS = ("arm64-v8a", "x86_64")


def normalize_certificate_digest(value: str) -> str:
    normalized = value.replace(":", "").lower()
    if not CERTIFICATE_SHA256.fullmatch(normalized):
        raise ValueError("Android signing certificate SHA-256 must contain 64 hex digits")
    return normalized


def validate_package(path: Path, required_entries: set[str]) -> None:
    size = validate_regular_file(path)
    if size < 4096:
        raise ValueError(f"Android release package is implausibly small: {path.name}")
    try:
        with zipfile.ZipFile(path) as package:
            entries = package.namelist()
            duplicate_entries = sorted(
                name for name, count in Counter(entries).items() if count > 1
            )
            if duplicate_entries:
                raise ValueError(
                    f"{path.name} contains duplicate ZIP entries: "
                    f"{', '.join(duplicate_entries)}"
                )
            missing = sorted(required_entries - set(entries))
            if missing:
                raise ValueError(
                    f"{path.name} is missing required entries: {', '.join(missing)}"
                )
            if any(name.endswith("/libenvoix_jni.so") for name in entries):
                raise ValueError(f"{path.name} contains the retired libenvoix_jni.so")
            corrupt = package.testzip()
            if corrupt is not None:
                raise ValueError(f"{path.name} has a corrupt ZIP entry: {corrupt}")
    except zipfile.BadZipFile as error:
        raise ValueError(f"{path.name} is not a valid ZIP package") from error


def normalize_and_validate_sbom(
    path: Path,
    component_name: str,
    spec_version: str,
    version: str,
    repository: str,
    revision: str,
) -> None:
    size = validate_regular_file(path)
    if size > MAX_SBOM_BYTES:
        raise ValueError(f"{path.name} exceeds the 16 MiB attestation limit")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid SBOM JSON in {path.name}: {error}") from error

    expected_serial = sbom_serial(repository, revision, component_name)
    existing_serial = document.get("serialNumber")
    if existing_serial not in (None, expected_serial):
        raise ValueError(f"{path.name} has an unexpected serialNumber")
    metadata = document.get("metadata")
    if not isinstance(metadata, dict):
        raise ValueError(f"{path.name} has no metadata object")
    if document.get("bomFormat") != "CycloneDX":
        raise ValueError(f"{path.name} is not a CycloneDX SBOM")
    if document.get("specVersion") != spec_version:
        raise ValueError(f"{path.name} must use CycloneDX {spec_version}")
    if document.get("version") != 1:
        raise ValueError(f"{path.name} must have document version 1")
    component = metadata.get("component")
    if not isinstance(component, dict):
        raise ValueError(f"{path.name} has no metadata component")
    if component.get("name") != component_name:
        raise ValueError(f"{path.name} does not describe {component_name}")
    if component.get("version") != version:
        raise ValueError(
            f"{path.name} component version is {component.get('version')!r}, "
            f"expected {version!r}"
        )
    if not document.get("components") or not document.get("dependencies"):
        raise ValueError(f"{path.name} must include components and dependencies")

    document["serialNumber"] = expected_serial
    metadata.pop("timestamp", None)
    path.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def prepare(
    directory: Path,
    version: str,
    build_number: int,
    repository: str,
    revision: str,
    certificate_sha256: str,
) -> list[Artifact]:
    if not VERSION.fullmatch(version):
        raise ValueError(f"release version must be X.Y.Z, found {version!r}")
    if build_number <= 0:
        raise ValueError("Android versionCode must be positive")
    if not REPOSITORY.fullmatch(repository):
        raise ValueError(f"repository must be owner/name, found {repository!r}")
    if not GIT_COMMIT.fullmatch(revision):
        raise ValueError("revision must be a lowercase 40-character Git commit")
    certificate_sha256 = normalize_certificate_digest(certificate_sha256)

    directory = directory.resolve()
    if not directory.is_dir():
        raise ValueError(f"release directory does not exist: {directory}")
    expected = set(ANDROID_PACKAGES) | set(ANDROID_SBOMS)
    actual = {path.name for path in directory.iterdir()}
    missing = sorted(expected - actual)
    unexpected = sorted(actual - expected - set(GENERATED_FILES))
    if missing:
        raise ValueError(f"Android release bundle is missing: {', '.join(missing)}")
    if unexpected:
        raise ValueError(
            f"Android release bundle has unexpected files: {', '.join(unexpected)}"
        )

    apk_entries = {"AndroidManifest.xml", "classes.dex"}
    apk_entries.update(f"lib/{abi}/libenvoix_ffi.so" for abi in ANDROID_ABIS)
    aab_entries = {"base/manifest/AndroidManifest.xml", "base/dex/classes.dex"}
    aab_entries.update(f"base/lib/{abi}/libenvoix_ffi.so" for abi in ANDROID_ABIS)
    validate_package(directory / ANDROID_PACKAGES[0], apk_entries)
    validate_package(directory / ANDROID_PACKAGES[1], aab_entries)
    for name, (component_name, spec_version) in ANDROID_SBOMS.items():
        normalize_and_validate_sbom(
            directory / name,
            component_name,
            spec_version,
            version,
            repository,
            revision,
        )

    artifacts = []
    for name in sorted(expected):
        path = directory / name
        artifacts.append(
            Artifact(
                name=name,
                kind="sbom" if name in ANDROID_SBOMS else "android-package",
                size=path.stat().st_size,
                sha256=sha256(path),
            )
        )

    manifest_path = directory / GENERATED_FILES[0]
    manifest = {
        "schemaVersion": 1,
        "releaseVersion": version,
        "source": {"repository": repository, "revision": revision},
        "android": {
            "applicationId": APPLICATION_ID,
            "versionCode": build_number,
            "signingCertificateSha256": certificate_sha256,
        },
        "artifacts": [artifact.__dict__ for artifact in artifacts],
    }
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    checksummed = artifacts + [
        Artifact(
            name=manifest_path.name,
            kind="manifest",
            size=manifest_path.stat().st_size,
            sha256=sha256(manifest_path),
        )
    ]
    (directory / GENERATED_FILES[1]).write_text(
        "".join(f"{artifact.sha256}  {artifact.name}\n" for artifact in checksummed),
        encoding="ascii",
    )
    return artifacts


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build-number", required=True, type=int)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--certificate-sha256", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        artifacts = prepare(
            args.directory,
            args.version,
            args.build_number,
            args.repository,
            args.revision,
            args.certificate_sha256,
        )
    except ValueError as error:
        print(f"Android release bundle error: {error}", file=sys.stderr)
        return 1
    print(f"Android release bundle ok: {len(artifacts)} artifacts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
