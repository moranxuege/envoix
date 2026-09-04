#!/usr/bin/env python3
"""Validate and describe the immutable Envoix desktop release bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path


GIT_COMMIT = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")

LINUX_MAGIC = (b"\x7fELF",)
MACOS_MAGIC = (
    b"\xfe\xed\xfa\xce",
    b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa\xcf",
    b"\xcf\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe",
    b"\xbe\xba\xfe\xca",
)
WINDOWS_MAGIC = (b"MZ",)

BINARY_FORMATS = {
    "envoix-cli-linux-x86_64": LINUX_MAGIC,
    "envoix-agent-linux-x86_64": LINUX_MAGIC,
    "envoix-broker-linux-x86_64": LINUX_MAGIC,
    "envoix-cli-macos-aarch64": MACOS_MAGIC,
    "envoix-cli-macos-x86_64": MACOS_MAGIC,
    "envoix-cli-windows-x86_64.exe": WINDOWS_MAGIC,
    "envoix-agent-windows-x86_64.exe": WINDOWS_MAGIC,
    "Envoix-Windows-x86_64.exe": WINDOWS_MAGIC,
}
SBOM_COMPONENTS = {
    "envoix-cli.cdx.json": "envoix",
    "envoix-agent.cdx.json": "envoix-agent",
    "envoix-broker.cdx.json": "envoix-rendezvous-server",
    "envoix-windows.cdx.json": "envoix-windows",
}
GENERATED_FILES = ("release-manifest.json", "SHA256SUMS")
MAX_SBOM_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class Artifact:
    name: str
    kind: str
    size: int
    sha256: str


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_regular_file(path: Path) -> int:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"release artifact must be a regular file: {path.name}")
    size = path.stat().st_size
    if size == 0:
        raise ValueError(f"release artifact must not be empty: {path.name}")
    return size


def validate_binary(path: Path, accepted_magic: tuple[bytes, ...]) -> None:
    size = validate_regular_file(path)
    with path.open("rb") as binary:
        prefix = binary.read(max(map(len, accepted_magic)))
    if not any(prefix.startswith(magic) for magic in accepted_magic):
        raise ValueError(f"release binary has an unexpected file format: {path.name}")
    if size < 4096:
        raise ValueError(f"release binary is implausibly small: {path.name}")


def sbom_serial(repository: str, revision: str, component_name: str) -> str:
    identity = f"https://github.com/{repository}/commit/{revision}#{component_name}"
    return f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, identity)}"


def stamp_sbom_serial(
    path: Path, component_name: str, repository: str, revision: str
) -> str:
    size = validate_regular_file(path)
    if size > MAX_SBOM_BYTES:
        raise ValueError(f"{path.name} exceeds the 16 MiB attestation limit")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid SBOM JSON in {path.name}: {error}") from error
    expected = sbom_serial(repository, revision, component_name)
    existing = document.get("serialNumber")
    if existing not in (None, expected):
        raise ValueError(f"{path.name} has an unexpected serialNumber")
    if existing is None:
        document["serialNumber"] = expected
        path.write_text(
            json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    return expected


def validate_sbom(
    path: Path, component_name: str, version: str, expected_serial: str
) -> None:
    validate_regular_file(path)
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid SBOM JSON in {path.name}: {error}") from error
    if document.get("bomFormat") != "CycloneDX":
        raise ValueError(f"{path.name} is not a CycloneDX SBOM")
    if document.get("specVersion") != "1.5":
        raise ValueError(f"{path.name} must use CycloneDX 1.5")
    if document.get("version") != 1:
        raise ValueError(f"{path.name} must have document version 1")
    if document.get("serialNumber") != expected_serial:
        raise ValueError(f"{path.name} has an invalid release serialNumber")
    component = document.get("metadata", {}).get("component", {})
    if component.get("name") != component_name:
        raise ValueError(
            f"{path.name} describes {component.get('name')!r}, expected {component_name!r}"
        )
    if component.get("version") != version:
        raise ValueError(
            f"{path.name} component version is {component.get('version')!r}, expected {version!r}"
        )
    if not document.get("components") or not document.get("dependencies"):
        raise ValueError(f"{path.name} must include components and dependency relationships")


def validate_inputs(
    directory: Path, version: str, repository: str, revision: str
) -> list[Artifact]:
    if not VERSION.fullmatch(version):
        raise ValueError(f"release version must be X.Y.Z, found {version!r}")
    if not REPOSITORY.fullmatch(repository):
        raise ValueError(f"repository must be owner/name, found {repository!r}")
    if not GIT_COMMIT.fullmatch(revision):
        raise ValueError("revision must be a lowercase 40-character Git commit")
    if not directory.is_dir():
        raise ValueError(f"release directory does not exist: {directory}")

    expected = set(BINARY_FORMATS) | set(SBOM_COMPONENTS)
    actual = {path.name for path in directory.iterdir()}
    unexpected = sorted(actual - expected - set(GENERATED_FILES))
    missing = sorted(expected - actual)
    if missing:
        raise ValueError(f"release bundle is missing: {', '.join(missing)}")
    if unexpected:
        raise ValueError(f"release bundle has unexpected files: {', '.join(unexpected)}")

    for name, accepted_magic in BINARY_FORMATS.items():
        validate_binary(directory / name, accepted_magic)
    for name, component_name in SBOM_COMPONENTS.items():
        path = directory / name
        expected_serial = stamp_sbom_serial(
            path, component_name, repository, revision
        )
        validate_sbom(path, component_name, version, expected_serial)

    artifacts = []
    for name in sorted(expected):
        path = directory / name
        artifacts.append(
            Artifact(
                name=name,
                kind="sbom" if name in SBOM_COMPONENTS else "binary",
                size=path.stat().st_size,
                sha256=sha256(path),
            )
        )
    return artifacts


def write_bundle(
    directory: Path,
    version: str,
    repository: str,
    revision: str,
    artifacts: list[Artifact],
) -> None:
    manifest_path = directory / GENERATED_FILES[0]
    manifest = {
        "schemaVersion": 1,
        "releaseVersion": version,
        "source": {"repository": repository, "revision": revision},
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
    checksums_path = directory / GENERATED_FILES[1]
    checksums_path.write_text(
        "".join(f"{artifact.sha256}  {artifact.name}\n" for artifact in checksummed),
        encoding="ascii",
    )


def prepare(directory: Path, version: str, repository: str, revision: str) -> list[Artifact]:
    directory = directory.resolve()
    artifacts = validate_inputs(directory, version, repository, revision)
    write_bundle(directory, version, repository, revision, artifacts)
    return artifacts


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--revision", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        artifacts = prepare(
            args.directory, args.version, args.repository, args.revision
        )
    except ValueError as error:
        print(f"release bundle error: {error}", file=sys.stderr)
        return 1
    print(
        f"release bundle ok: version={args.version} "
        f"artifacts={len(artifacts)} revision={args.revision}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
