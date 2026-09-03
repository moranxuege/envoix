from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import release_contract  # noqa: E402


PINNED_ACTION = "0123456789abcdef0123456789abcdef01234567"


class ReleaseContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.write(
            "Cargo.toml",
            '[workspace]\n[workspace.package]\nversion = "0.3.0"\n',
        )
        self.write(
            "android/app/build.gradle.kts",
            'versionCode = 5\nversionName = "0.3.0"\n',
        )
        self.write(
            "apps/envoix-apple/project.yml",
            'MARKETING_VERSION: "0.3.0"\nCURRENT_PROJECT_VERSION: "5"\n',
        )
        self.write(
            "scripts/apple-dev.sh",
            'artifact="Envoix-0.3.0-macos-notarized.zip"\n',
        )
        self.write(
            ".github/workflows/ci.yml",
            f"steps:\n  - uses: actions/checkout@{PINNED_ACTION} # v4\n",
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, relative: str, contents: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents, encoding="utf-8")

    def test_valid_contract_returns_versions_and_action_count(self) -> None:
        contract = release_contract.validate(self.root, "v0.3.0")

        self.assertEqual(contract.version, "0.3.0")
        self.assertEqual(contract.build_number, 5)
        self.assertEqual(contract.action_count, 1)

    def test_mutable_action_tag_is_rejected(self) -> None:
        self.write(
            ".github/workflows/ci.yml",
            "steps:\n  - uses: actions/checkout@v4\n",
        )

        with self.assertRaisesRegex(ValueError, "40-character commit SHA"):
            release_contract.validate(self.root)

    def test_cross_platform_version_drift_is_rejected(self) -> None:
        self.write(
            "android/app/build.gradle.kts",
            'versionCode = 5\nversionName = "0.3.1"\n',
        )

        with self.assertRaisesRegex(ValueError, "release versions disagree"):
            release_contract.validate(self.root)

    def test_cross_platform_build_number_drift_is_rejected(self) -> None:
        self.write(
            "android/app/build.gradle.kts",
            'versionCode = 6\nversionName = "0.3.0"\n',
        )

        with self.assertRaisesRegex(ValueError, "release build numbers disagree"):
            release_contract.validate(self.root)

    def test_wrong_release_tag_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "release tag must be v0.3.0"):
            release_contract.validate(self.root, "v0.3.1")


if __name__ == "__main__":
    unittest.main()
