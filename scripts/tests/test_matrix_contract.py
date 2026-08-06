from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import matrix_contract  # noqa: E402


REGISTRY_PATH = REPO_ROOT / "tests/e2e/matrix/cases.v1.json"
RUNNER_FIXTURE_PATH = REPO_ROOT / "tests/e2e/matrix/fixtures/runner-results.v1.json"


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

    def test_network_profiles_are_required(self) -> None:
        errors = self.errors_for(lambda value: value.pop("network_profiles"))
        self.assert_error(errors, "missing field(s): network_profiles")

    def test_case_referencing_unknown_network_profile_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__("network_profile", "no_such")
        )
        self.assert_error(errors, "network_profile references unknown profile 'no_such'")

    def test_network_profile_bounds_are_enforced(self) -> None:
        for field, bad in (
            ("downlink_kbits", 0),
            ("uplink_kbits", -1),
            ("rtt_ms", 0),
            ("loss_percent", 90.0),
        ):
            with self.subTest(field=field):
                errors = self.errors_for(
                    lambda value, f=field, b=bad: value["network_profiles"][
                        "home_wifi"
                    ].__setitem__(f, b)
                )
                self.assert_error(errors, f"network_profiles.home_wifi.{field}")

    def test_unknown_nat_profile_is_rejected(self) -> None:
        errors = self.errors_for(
            lambda value: value["cases"][0].__setitem__("nat_profile", "double_nat")
        )
        self.assert_error(errors, "nat_profile has unsupported value")

    def test_every_case_states_its_link_and_translation(self) -> None:
        # A speed row without a stated link means nothing, and a NAT row without
        # stated translation cannot be reproduced.
        for case in self.registry["cases"]:
            self.assertIn(case["network_profile"], self.registry["network_profiles"])
            self.assertIn(case["nat_profile"], matrix_contract.NAT_PROFILES)

    def test_throughput_is_recorded_and_checked(self) -> None:
        record = matrix_contract.execution_record(
            run_id="speed-1",
            case_id="l1.emulator.speed.friendly-both-ipv4.home-wifi",
            repetition=1,
            status="pass",
            throughput={"bytes": 8388608, "seconds": 4.0, "kib_per_second": 2048.0},
        )
        self.assertEqual(record["throughput"]["kib_per_second"], 2048.0)
        with self.assertRaises(ValueError):
            matrix_contract.execution_record(
                run_id="speed-1",
                case_id="l1.emulator.speed.friendly-both-ipv4.home-wifi",
                repetition=1,
                status="pass",
                throughput={"bytes": 1, "seconds": 0, "kib_per_second": 1},
            )

    def test_repository_registry_is_valid(self) -> None:
        self.assertEqual(matrix_contract.validate_registry(self.registry), [])
        self.assertEqual(len(self.registry["cases"]), 32)
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

    def test_case_resolution_preserves_registry_order_without_expansion(self) -> None:
        selected, warnings, selection = matrix_contract.resolve_cases(
            self.registry,
            case_ids=[
                "l1.physical.room.android-ios.multiple-files",
                "l1.physical.room.ios-android.single-file",
            ],
        )
        self.assertEqual(warnings, [])
        self.assertEqual(selection, "case")
        self.assertEqual(
            [case["case_id"] for case in selected],
            [
                "l1.physical.room.ios-android.single-file",
                "l1.physical.room.android-ios.multiple-files",
            ],
        )

    def test_legacy_resolution_warns_for_unregistered_combinations(self) -> None:
        selected, warnings, selection = matrix_contract.resolve_cases(
            self.registry,
            legacy_scenarios=["single_file", "image"],
            legacy_directions=["ios:android", "macos:ios"],
        )
        self.assertEqual(selection, "legacy")
        self.assertEqual(
            [case["case_id"] for case in selected],
            ["l1.physical.room.ios-android.single-file"],
        )
        self.assertEqual(len(warnings), 3)

    def test_gate_and_tag_resolution_select_only_explicit_rows(self) -> None:
        gate_cases, _, gate_selection = matrix_contract.resolve_cases(
            self.registry,
            gate="cross-platform-recovery",
        )
        tag_cases, _, tag_selection = matrix_contract.resolve_cases(
            self.registry,
            tag="recovery",
        )
        expected = [
            "l2.recovery.room.ios-android.network-interrupt",
            "l2.recovery.room.android-ios.network-interrupt",
        ]
        self.assertEqual([case["case_id"] for case in gate_cases], expected)
        self.assertEqual([case["case_id"] for case in tag_cases], expected)
        self.assertEqual(gate_selection, "gate:cross-platform-recovery")
        self.assertEqual(tag_selection, "tag:recovery")

    def test_mixed_selection_modes_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "select exactly one"):
            matrix_contract.resolve_cases(
                self.registry,
                case_ids=["l1.physical.room.ios-android.single-file"],
                gate="current-physical-harness",
            )

    def test_dry_run_plan_is_never_executable(self) -> None:
        selected, _, selection = matrix_contract.resolve_cases(
            self.registry,
            gate="current-physical-harness",
        )
        plan = matrix_contract.build_run_plan(
            self.registry,
            selected,
            run_id="fixture-run",
            tested_commit="0123456789abcdef",
            build_variant="debug",
            selection=selection,
            dry_run=True,
        )
        self.assertTrue(plan["dry_run"])
        self.assertEqual(len(plan["executions"]), 12)
        self.assertEqual(
            {execution["disposition"] for execution in plan["executions"]},
            {"not_run"},
        )
        self.assertEqual(matrix_contract.validate_run_plan(plan, self.registry), [])

    def test_repetition_override_cannot_weaken_the_registry(self) -> None:
        selected, _, selection = matrix_contract.resolve_cases(
            self.registry,
            case_ids=["l1.physical.room.ios-android.single-file"],
        )
        with self.assertRaisesRegex(ValueError, "requires at least 2 repetitions"):
            matrix_contract.build_run_plan(
                self.registry,
                selected,
                run_id="fixture-run",
                tested_commit="0123456789abcdef",
                build_variant="debug",
                selection=selection,
                dry_run=False,
                repetitions=1,
            )

    def test_plan_cannot_execute_a_planned_case(self) -> None:
        selected, _, selection = matrix_contract.resolve_cases(
            self.registry,
            gate="cross-platform-recovery",
        )
        plan = matrix_contract.build_run_plan(
            self.registry,
            selected,
            run_id="fixture-run",
            tested_commit="0123456789abcdef",
            build_variant="release_equivalent",
            selection=selection,
            dry_run=False,
        )
        plan["executions"][0]["disposition"] = "execute"
        self.assert_error(
            matrix_contract.validate_run_plan(plan, self.registry),
            "disposition does not match support and run state",
        )

    def test_fixture_aggregate_is_deterministic_and_keeps_failure_classes(self) -> None:
        fixture = json.loads(RUNNER_FIXTURE_PATH.read_text(encoding="utf-8"))
        selected, _, selection = matrix_contract.resolve_cases(
            self.registry,
            case_ids=fixture["case_ids"],
        )
        plan = matrix_contract.build_run_plan(
            self.registry,
            selected,
            run_id="fixture-run",
            tested_commit="0123456789abcdef",
            build_variant="debug",
            selection=selection,
            dry_run=False,
        )
        with tempfile.TemporaryDirectory() as directory:
            records_root = Path(directory)
            for raw_record in fixture["records"]:
                record = matrix_contract.execution_record(
                    run_id=plan["run_id"],
                    **raw_record,
                )
                execution = next(
                    execution
                    for execution in plan["executions"]
                    if execution["case_id"] == raw_record["case_id"]
                    and execution["repetition"] == raw_record["repetition"]
                )
                path = matrix_contract._record_path(records_root, execution)
                path.parent.mkdir(parents=True)
                path.write_text(
                    json.dumps(record, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8",
                )
            first = matrix_contract.aggregate_run(self.registry, plan, records_root)
            second = matrix_contract.aggregate_run(self.registry, plan, records_root)

        self.assertEqual(first, second)
        self.assertEqual(first["summary"]["product_failure"], 1)
        self.assertEqual(first["summary"]["infrastructure_failure"], 1)
        self.assertEqual(first["run_status"], "failure")

    def test_missing_execution_record_is_infrastructure_failure(self) -> None:
        selected, _, selection = matrix_contract.resolve_cases(
            self.registry,
            case_ids=["l1.physical.room.ios-android.single-file"],
        )
        plan = matrix_contract.build_run_plan(
            self.registry,
            selected,
            run_id="fixture-run",
            tested_commit="0123456789abcdef",
            build_variant="debug",
            selection=selection,
            dry_run=False,
        )
        with tempfile.TemporaryDirectory() as directory:
            aggregate = matrix_contract.aggregate_run(
                self.registry,
                plan,
                Path(directory),
            )
        self.assertEqual(aggregate["summary"]["infrastructure_failure"], 2)
        self.assertTrue(
            all(
                result["classification"] == "infrastructure"
                for result in aggregate["results"]
            )
        )

    def test_redaction_removes_public_canaries(self) -> None:
        raw = (
            "pairing_code=741203-amber-comet "
            "envoix://invite/v2/canary "
            "serial=ANDROID_SERIAL_CANARY "
            "/private/canary/path "
            "192.0.2.10"
        )
        redacted = matrix_contract.redact_text(raw)
        self.assertEqual(matrix_contract.sensitive_findings(redacted), [])
        for canary in (
            "741203-amber-comet",
            "envoix://invite/v2/canary",
            "ANDROID_SERIAL_CANARY",
            "/private/canary/path",
            "192.0.2.10",
        ):
            self.assertNotIn(canary, redacted)

    def test_runner_dry_run_output_is_deterministic_and_never_passes(self) -> None:
        runner = REPO_ROOT / "scripts/cross-device-transfer-matrix.sh"

        def run(output: Path) -> dict[str, str]:
            subprocess.run(
                [
                    str(runner),
                    "--dry-run",
                    "--case",
                    "l1.physical.room.ios-android.single-file",
                    "--run-id",
                    "fixture-run",
                    "--commit",
                    "0123456789abcdef",
                    "--output-directory",
                    str(output),
                ],
                cwd=REPO_ROOT,
                check=True,
                capture_output=True,
                text=True,
            )
            return {
                str(path.relative_to(output)): path.read_text(encoding="utf-8")
                for path in sorted(output.rglob("*"))
                if path.is_file()
            }

        with tempfile.TemporaryDirectory() as first_directory:
            with tempfile.TemporaryDirectory() as second_directory:
                first = run(Path(first_directory))
                second = run(Path(second_directory))

        self.assertEqual(first, second)
        result = json.loads(first["matrix-result.json"])
        self.assertEqual(result["summary"]["pass"], 0)
        self.assertEqual(result["summary"]["not_run"], 2)
        self.assertTrue(result["dry_run"])

    def test_runner_records_missing_device_input_as_infrastructure_failure(
        self,
    ) -> None:
        runner = REPO_ROOT / "scripts/cross-device-transfer-matrix.sh"
        environment = os.environ.copy()
        environment["ENVOIX_BUILD_LEASE_HELD"] = "1"
        environment.pop("ENVOIX_IOS_DESTINATION", None)
        with tempfile.TemporaryDirectory() as directory:
            completed = subprocess.run(
                [
                    str(runner),
                    "--case",
                    "l1.physical.room.ios-android.single-file",
                    "--run-id",
                    "missing-device-run",
                    "--commit",
                    "0123456789abcdef",
                    "--output-directory",
                    directory,
                ],
                cwd=REPO_ROOT,
                env=environment,
                capture_output=True,
                text=True,
            )
            result = json.loads(
                (Path(directory) / "matrix-result.json").read_text(encoding="utf-8")
            )

        self.assertEqual(completed.returncode, 1)
        self.assertEqual(result["summary"]["infrastructure_failure"], 2)
        self.assertEqual(result["summary"]["product_failure"], 0)
        self.assertEqual(result["run_status"], "failure")


if __name__ == "__main__":
    unittest.main()
