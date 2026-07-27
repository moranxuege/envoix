from __future__ import annotations

import copy
import json
import sys
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import matrix_contract  # noqa: E402


REGISTRY_PATH = REPO_ROOT / "tests/e2e/matrix/cases.v1.json"


class MatrixContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.registry = json.loads(REGISTRY_PATH.read_text(encoding="utf-8"))

    def errors_for(self, transform) -> list[str]:
        candidate = copy.deepcopy(self.registry)
        transform(candidate)
        return matrix_contract.validate_registry(candidate)

    def assert_error(self, errors: list[str], expected: str) -> None:
        self.assertTrue(
            any(expected in error for error in errors),
            f"expected {expected!r} in errors:\n" + "\n".join(errors),
        )

    def test_repository_registry_is_valid(self) -> None:
        self.assertEqual(matrix_contract.validate_registry(self.registry), [])
        self.assertEqual(len(self.registry["cases"]), 22)
        self.assertFalse(
            any(case["support_status"] == "required" for case in self.registry["cases"])
        )

    def test_unknown_case_field_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__("surprise", True)
        )
        self.assert_error(errors, "unknown field(s): surprise")

    def test_duplicate_case_id_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][1].__setitem__(
                "case_id",
                value["cases"][0]["case_id"],
            )
        )
        self.assert_error(errors, "duplicate case_id")

    def test_non_passing_support_status_requires_reason(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__("support_reason", None)
        )
        self.assert_error(errors, "support_reason must be a string")

    def test_supported_case_rejects_support_reason(self) -> None:
        def transform(value) -> None:
            value["cases"][0]["support_status"] = "supported"

        errors = self.errors_for(transform)
        self.assert_error(errors, "support_reason must be null")

    def test_l2_case_requires_three_repetitions(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][6].__setitem__("required_repetitions", 2)
        )
        self.assert_error(errors, "L2 cases require at least three repetitions")

    def test_completed_l2_case_requires_publication_evidence(self) -> None:
        def transform(value) -> None:
            value["cases"][6]["required_evidence"].remove("publication")

        errors = self.errors_for(transform)
        self.assert_error(errors, "completed L2 evidence is missing: publication")

    def test_network_recovery_requires_reused_progress(self) -> None:
        def transform(value) -> None:
            case = next(
                case
                for case in value["cases"]
                if case["fault_profile"] == "network_interrupt"
            )
            case["required_evidence"].remove("resume_reused_bytes")

        errors = self.errors_for(transform)
        self.assert_error(errors, "recovery cases require resume_reused_bytes")

    def test_failed_case_requires_structured_failure_evidence(self) -> None:
        def transform(value) -> None:
            case = next(
                case
                for case in value["cases"]
                if case["expected_terminal_state"] == "failed"
            )
            case["required_evidence"].remove("failure_phase")

        errors = self.errors_for(transform)
        self.assert_error(errors, "failed-case evidence is missing: failure_phase")

    def test_future_carrier_cannot_be_promoted_without_contract_change(self) -> None:
        def transform(value) -> None:
            case = next(
                case for case in value["cases"] if case["invitation_input"] == "ble"
            )
            case["support_status"] = "supported"
            case["support_reason"] = None

        errors = self.errors_for(transform)
        self.assert_error(errors, "while ble is a future carrier")

    def test_secret_bearing_field_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__(
                "room_code",
                "741203-amber-comet",
            )
        )
        self.assert_error(errors, "contains prohibited key 'room_code'")

    def test_room_code_shaped_value_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__(
                "support_reason",
                "Pairing used 741203-amber-comet.",
            )
        )
        self.assert_error(errors, "contains sensitive Room Code-shaped secret")

    def test_invitation_uri_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__(
                "support_reason",
                "Captured envoix://invite/v2/canary.",
            )
        )
        self.assert_error(errors, "contains sensitive invitation URI")

    def test_absolute_private_path_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__(
                "support_reason",
                "Output remained under /Users/canary/private.",
            )
        )
        self.assert_error(errors, "contains sensitive absolute private path")

    def test_unknown_transfer_profile_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__(
                "transfer_profile",
                "missing_profile",
            )
        )
        self.assert_error(errors, "references unknown profile")

    def test_case_selection_uses_explicit_filters(self) -> None:
        selected = matrix_contract.select_cases(
            self.registry,
            statuses={"planned"},
            gate="cross-platform-baseline",
            tag="multi-entry",
        )
        self.assertEqual(
            [case["case_id"] for case in selected],
            [
                "l2.baseline.room.ios-android.multiple-files",
                "l2.baseline.room.android-ios.multiple-files",
            ],
        )

    def test_registry_report_never_turns_not_run_into_pass(self) -> None:
        selected = matrix_contract.select_cases(
            self.registry,
            statuses={"experimental"},
        )
        first = matrix_contract.render_registry_report(self.registry, selected)
        second = matrix_contract.render_registry_report(self.registry, selected)
        self.assertEqual(first, second)
        self.assertEqual(first.count("| not_run |"), len(selected))
        self.assertNotIn("| pass |", first)
        self.assertIn("`not_run` is never a pass", first)


if __name__ == "__main__":
    unittest.main()
