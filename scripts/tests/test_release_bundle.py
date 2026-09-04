from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import release_bundle  # noqa: E402


VERSION = "0.3.0"
REPOSITORY = "moranxuege/envoix"
REVISION = "0123456789abcdef0123456789abcdef01234567"


class ReleaseBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        for name, magic in release_bundle.BINARY_FORMATS.items():
            (self.directory / name).write_bytes(magic[0] + b"\0" * 4096)
        for name, component in release_bundle.SBOM_COMPONENTS.items():
            (self.directory / name).write_text(
                json.dumps(
                    {
                        "bomFormat": "CycloneDX",
                        "specVersion": "1.5",
                        "version": 1,
                        "metadata": {
                            "component": {"name": component, "version": VERSION}
                        },
                        "components": [{"name": "dependency"}],
                        "dependencies": [{"ref": "dependency", "dependsOn": []}],
                    }
                ),
                encoding="utf-8",
            )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_prepare_writes_deterministic_manifest_and_checksums(self) -> None:
        artifacts = release_bundle.prepare(
            self.directory, VERSION, REPOSITORY, REVISION
        )

        self.assertEqual(len(artifacts), 12)
        manifest_path = self.directory / "release-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        self.assertEqual(manifest["releaseVersion"], VERSION)
        self.assertEqual(manifest["source"]["revision"], REVISION)
        self.assertEqual(
            [artifact["name"] for artifact in manifest["artifacts"]],
            sorted(set(release_bundle.BINARY_FORMATS) | set(release_bundle.SBOM_COMPONENTS)),
        )
        checksum_lines = (self.directory / "SHA256SUMS").read_text().splitlines()
        self.assertEqual(len(checksum_lines), 13)
        expected_manifest_digest = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
        self.assertIn(
            f"{expected_manifest_digest}  release-manifest.json", checksum_lines
        )
        cli_sbom = json.loads(
            (self.directory / "envoix-cli.cdx.json").read_text(encoding="utf-8")
        )
        self.assertEqual(
            cli_sbom["serialNumber"],
            release_bundle.sbom_serial(REPOSITORY, REVISION, "envoix"),
        )

    def test_prepare_is_repeatable(self) -> None:
        release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)
        first_manifest = (self.directory / "release-manifest.json").read_bytes()
        first_checksums = (self.directory / "SHA256SUMS").read_bytes()

        release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

        self.assertEqual(
            (self.directory / "release-manifest.json").read_bytes(), first_manifest
        )
        self.assertEqual((self.directory / "SHA256SUMS").read_bytes(), first_checksums)

    def test_missing_binary_is_rejected(self) -> None:
        (self.directory / "envoix-cli-linux-x86_64").unlink()

        with self.assertRaisesRegex(ValueError, "release bundle is missing"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

    def test_unexpected_file_is_rejected(self) -> None:
        (self.directory / "surprise.txt").write_text("unexpected", encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "unexpected files"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

    def test_wrong_binary_format_is_rejected(self) -> None:
        (self.directory / "envoix-cli-windows-x86_64.exe").write_bytes(b"NO" * 4096)

        with self.assertRaisesRegex(ValueError, "unexpected file format"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

    def test_wrong_sbom_component_is_rejected(self) -> None:
        path = self.directory / "envoix-agent.cdx.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        document["metadata"]["component"]["name"] = "envoix"
        path.write_text(json.dumps(document), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "describes 'envoix'"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

    def test_foreign_sbom_serial_is_rejected(self) -> None:
        path = self.directory / "envoix-agent.cdx.json"
        document = json.loads(path.read_text(encoding="utf-8"))
        document["serialNumber"] = "urn:uuid:00000000-0000-4000-8000-000000000000"
        path.write_text(json.dumps(document), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "unexpected serialNumber"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, REVISION)

    def test_revision_must_be_a_commit(self) -> None:
        with self.assertRaisesRegex(ValueError, "40-character Git commit"):
            release_bundle.prepare(self.directory, VERSION, REPOSITORY, "main")


if __name__ == "__main__":
    unittest.main()
