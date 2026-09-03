from __future__ import annotations

import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import android_release_bundle  # noqa: E402
from release_bundle import sbom_serial  # noqa: E402


VERSION = "0.3.0"
BUILD_NUMBER = 5
REPOSITORY = "moranxuege/envoix"
REVISION = "0123456789abcdef0123456789abcdef01234567"
CERTIFICATE = "01" * 32


class AndroidReleaseBundleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.directory = Path(self.temporary.name)
        self.write_package(
            "envoix-android.apk",
            {
                "AndroidManifest.xml",
                "classes.dex",
                "lib/arm64-v8a/libenvoix_ffi.so",
                "lib/x86_64/libenvoix_ffi.so",
            },
        )
        self.write_package(
            "envoix-android.aab",
            {
                "base/manifest/AndroidManifest.xml",
                "base/dex/classes.dex",
                "base/lib/arm64-v8a/libenvoix_ffi.so",
                "base/lib/x86_64/libenvoix_ffi.so",
            },
        )
        self.write_sbom("envoix-android.cdx.json", "envoix-android", "1.6")
        self.write_sbom("envoix-android-rust.cdx.json", "envoix-ffi", "1.5")

    def write_sbom(self, name: str, component_name: str, spec_version: str) -> None:
        (self.directory / name).write_text(
            json.dumps(
                {
                    "bomFormat": "CycloneDX",
                    "specVersion": spec_version,
                    "version": 1,
                    "metadata": {
                        "timestamp": "2026-01-01T00:00:00Z",
                        "component": {
                            "name": component_name,
                            "version": VERSION,
                        },
                    },
                    "components": [{"name": "dependency"}],
                    "dependencies": [{"ref": "dependency", "dependsOn": []}],
                }
            ),
            encoding="utf-8",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_package(self, name: str, entries: set[str]) -> None:
        with zipfile.ZipFile(self.directory / name, "w") as package:
            for entry in sorted(entries):
                package.writestr(entry, b"x" * 4096)

    def prepare(self) -> list[android_release_bundle.Artifact]:
        return android_release_bundle.prepare(
            self.directory,
            VERSION,
            BUILD_NUMBER,
            REPOSITORY,
            REVISION,
            CERTIFICATE,
        )

    def test_prepare_normalizes_sbom_and_writes_manifest(self) -> None:
        artifacts = self.prepare()

        self.assertEqual(len(artifacts), 4)
        sbom = json.loads(
            (self.directory / "envoix-android.cdx.json").read_text(encoding="utf-8")
        )
        self.assertNotIn("timestamp", sbom["metadata"])
        self.assertEqual(
            sbom["serialNumber"], sbom_serial(REPOSITORY, REVISION, "envoix-android")
        )
        rust_sbom = json.loads(
            (self.directory / "envoix-android-rust.cdx.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(
            rust_sbom["serialNumber"],
            sbom_serial(REPOSITORY, REVISION, "envoix-ffi"),
        )
        manifest = json.loads(
            (self.directory / "android-release-manifest.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertEqual(manifest["android"]["applicationId"], "dev.envoix.app")
        self.assertEqual(manifest["android"]["versionCode"], BUILD_NUMBER)
        self.assertEqual(
            manifest["android"]["signingCertificateSha256"], CERTIFICATE
        )
        self.assertEqual(
            len((self.directory / "SHA256SUMS.android").read_text().splitlines()), 5
        )

    def test_missing_abi_is_rejected(self) -> None:
        self.write_package(
            "envoix-android.aab",
            {
                "base/manifest/AndroidManifest.xml",
                "base/dex/classes.dex",
                "base/lib/arm64-v8a/libenvoix_ffi.so",
            },
        )

        with self.assertRaisesRegex(ValueError, "x86_64"):
            self.prepare()

    def test_retired_jni_library_is_rejected(self) -> None:
        path = self.directory / "envoix-android.apk"
        with zipfile.ZipFile(path, "a") as package:
            package.writestr("lib/arm64-v8a/libenvoix_jni.so", b"retired")

        with self.assertRaisesRegex(ValueError, "retired libenvoix_jni"):
            self.prepare()

    def test_foreign_sbom_serial_is_rejected(self) -> None:
        path = self.directory / "envoix-android.cdx.json"
        sbom = json.loads(path.read_text(encoding="utf-8"))
        sbom["serialNumber"] = "urn:uuid:00000000-0000-4000-8000-000000000000"
        path.write_text(json.dumps(sbom), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "unexpected serialNumber"):
            self.prepare()

    def test_invalid_sbom_is_not_rewritten(self) -> None:
        path = self.directory / "envoix-android.cdx.json"
        sbom = json.loads(path.read_text(encoding="utf-8"))
        sbom["specVersion"] = "1.5"
        path.write_text(json.dumps(sbom), encoding="utf-8")
        before = path.read_bytes()

        with self.assertRaisesRegex(ValueError, "CycloneDX 1.6"):
            self.prepare()

        self.assertEqual(path.read_bytes(), before)

    def test_invalid_certificate_digest_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "64 hex digits"):
            android_release_bundle.prepare(
                self.directory,
                VERSION,
                BUILD_NUMBER,
                REPOSITORY,
                REVISION,
                "not-a-certificate-digest",
            )

    def test_unexpected_file_is_rejected(self) -> None:
        (self.directory / "unsigned.apk").write_bytes(b"not expected")

        with self.assertRaisesRegex(ValueError, "unexpected files"):
            self.prepare()


if __name__ == "__main__":
    unittest.main()
