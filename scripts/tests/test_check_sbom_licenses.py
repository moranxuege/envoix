from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS_DIRECTORY = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS_DIRECTORY))

from check_sbom_licenses import validate_android_sbom  # noqa: E402


def sbom(*components: dict[str, object]) -> dict[str, object]:
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "metadata": {
            "component": {
                "group": "dev.envoix",
                "name": "envoix-android",
                "version": "0.3.0",
            }
        },
        "components": list(components),
    }


def component(name: str, *licenses: str) -> dict[str, object]:
    return {
        "name": name,
        "version": "1.0.0",
        "licenses": [{"license": {"id": identifier}} for identifier in licenses],
    }


class CheckSbomLicensesTests(unittest.TestCase):
    def validate(self, document: dict[str, object]) -> int:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bom.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return validate_android_sbom(path)

    def test_accepts_approved_and_dual_licensed_components(self) -> None:
        document = sbom(
            component("androidx", "Apache-2.0"),
            component("jna", "LGPL-2.1-or-later", "Apache-2.0"),
            component("camera", "Apache-2.0", "BSD-3-Clause"),
        )

        self.assertEqual(self.validate(document), 3)

    def test_rejects_component_without_approved_choice(self) -> None:
        document = sbom(component("copyleft-only", "LGPL-2.1-or-later"))

        with self.assertRaisesRegex(ValueError, "copyleft-only@1.0.0"):
            self.validate(document)

    def test_rejects_missing_license(self) -> None:
        document = sbom({"name": "unknown", "version": "1.0.0"})

        with self.assertRaisesRegex(ValueError, "has no declared license"):
            self.validate(document)

    def test_rejects_unaudited_expression(self) -> None:
        candidate = component("expression")
        candidate["licenses"] = [{"expression": "Apache-2.0 OR MIT"}]

        with self.assertRaisesRegex(ValueError, "unaudited SPDX expression"):
            self.validate(sbom(candidate))

    def test_rejects_wrong_root_component(self) -> None:
        document = sbom(component("androidx", "Apache-2.0"))
        document["metadata"]["component"]["name"] = "lookalike"  # type: ignore[index]

        with self.assertRaisesRegex(ValueError, "component must be"):
            self.validate(document)

    def test_rejects_duplicate_component_identity(self) -> None:
        repeated = component("androidx", "Apache-2.0")

        with self.assertRaisesRegex(ValueError, "repeats component"):
            self.validate(sbom(repeated, repeated))


if __name__ == "__main__":
    unittest.main()
