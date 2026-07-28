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

import apple_matrix_evidence  # noqa: E402
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

    def endpoint_result(self) -> dict:
        return {
            "schema_version": 1,
            "run_id": "fixture-run-c1-r1",
            "case_id": "l1.physical.room.android-ios.single-file",
            "repetition": 1,
            "role": "sender",
            "platform": "android",
            "test_layer": "l1_native",
            "driver": "direct_jni",
            "build_variant": "debug",
            "app_version": "0.2.0",
            "core_version": None,
            "protocol_version": 2,
            "device_model": "Android SDK built for arm64",
            "os_version": "Android 14 (API 34)",
            "capabilities": ["manifest_v2", "media_store_publication"],
            "activity_id": None,
            "job_id": "fixture-job",
            "started_at": 1_722_182_400_000,
            "finished_at": 1_722_182_401_000,
            "terminal_state": "completed",
            "ordered_phases": [
                "waiting_for_peer",
                "pairing",
                "connecting",
                "transferring",
                "waiting_for_receiver_save",
                "finalizing_delivery",
                "completed",
            ],
            "attempt_count": 1,
            "selected_path": "relay",
            "path_reason": None,
            "source_summary": {
                "root_count": 1,
                "file_count": 1,
                "directory_count": 0,
                "plaintext_bytes": 7,
                "manifest_digest": None,
                "tree_digest": "a" * 64,
                "entries": [
                    {
                        "relative_path": "single.txt",
                        "kind": "file",
                        "plaintext_bytes": 7,
                        "sha256": "b" * 64,
                        "disposition": "completed",
                    }
                ],
                "publication": None,
            },
            "destination_summary": None,
            "delivery_proof": True,
            "failure": None,
            "cleanup": {"test_owned": True, "completed": True},
            "metrics": {"plaintext_bytes": 7, "elapsed_ms": 1_000},
        }

    def product_endpoint_result(self, role: str = "sender") -> dict:
        value = self.endpoint_result()
        value["case_id"] = "l2.baseline.room.android-ios.single-file"
        value["test_layer"] = "l2_physical"
        value["driver"] = "product_activity"
        value["build_variant"] = "release_equivalent"
        value["activity_id"] = "activity-1"
        if role == "receiver":
            value["role"] = "receiver"
            value["source_summary"] = None
            value["destination_summary"] = {
                "root_count": 1,
                "file_count": 1,
                "directory_count": 0,
                "plaintext_bytes": 7,
                "manifest_digest": None,
                "tree_digest": "a" * 64,
                "entries": [
                    {
                        "relative_path": "single.txt",
                        "kind": "file",
                        "plaintext_bytes": 7,
                        "sha256": "b" * 64,
                        "disposition": "completed",
                    }
                ],
                "publication": {
                    "mechanism": "media_store",
                    "committed": True,
                },
            }
        return value

    def test_android_l1_endpoint_result_is_valid(self) -> None:
        self.assertEqual(
            matrix_contract.validate_endpoint_result(self.endpoint_result()),
            [],
        )

    def test_apple_l1_endpoint_result_is_valid(self) -> None:
        value = self.endpoint_result()
        value["platform"] = "ios"
        value["driver"] = "direct_ffi"
        value["device_model"] = "iPhone"
        value["os_version"] = "iOS 18.5"
        self.assertEqual(matrix_contract.validate_endpoint_result(value), [])

    def test_direct_ffi_endpoint_result_requires_apple_l1(self) -> None:
        value = self.endpoint_result()
        value["driver"] = "direct_ffi"
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "direct_ffi endpoint results must be Apple L1")

    def test_product_activity_endpoint_result_is_valid(self) -> None:
        self.assertEqual(
            matrix_contract.validate_endpoint_result(
                self.product_endpoint_result("sender")
            ),
            [],
        )
        self.assertEqual(
            matrix_contract.validate_endpoint_result(
                self.product_endpoint_result("receiver")
            ),
            [],
        )

    def test_product_activity_requires_release_activity_and_typed_path(self) -> None:
        value = self.product_endpoint_result()
        value["build_variant"] = "debug"
        value["activity_id"] = None
        value["selected_path"] = None
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "require a release-equivalent build")
        self.assert_error(errors, "require activity_id")
        self.assert_error(errors, "require selected_path")

    def test_product_activity_receiver_requires_native_publication(self) -> None:
        value = self.product_endpoint_result("receiver")
        value["destination_summary"]["publication"] = {
            "mechanism": "test_local_directory",
            "committed": True,
        }
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "cannot use test-local publication")

        value["destination_summary"]["publication"] = None
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "requires publication")

        value["destination_summary"]["publication"] = {
            "mechanism": "files_directory",
            "committed": False,
        }
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "requires committed publication")

    def test_apple_typed_failure_phases_are_valid(self) -> None:
        value = self.endpoint_result()
        value["platform"] = "macos"
        value["driver"] = "direct_ffi"
        value["terminal_state"] = "failed"
        value["ordered_phases"][-1] = "failed"
        value["delivery_proof"] = False
        value["failure"] = {
            "code": "authentication_failed",
            "phase": "authenticating",
            "recovery_action": "re_pair",
        }
        self.assertEqual(matrix_contract.validate_endpoint_result(value), [])

    def test_apple_attachment_extractor_selects_expected_identity(self) -> None:
        value = self.endpoint_result()
        value["platform"] = "ios"
        value["driver"] = "direct_ffi"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "manifest.json").write_text("{}", encoding="utf-8")
            attachment = root / "0_Test_envoix-matrix-sender.json"
            attachment.write_text(json.dumps(value), encoding="utf-8")
            selected = apple_matrix_evidence.find_endpoint_attachment(
                root,
                run_id=value["run_id"],
                case_id=value["case_id"],
                repetition=value["repetition"],
                role=value["role"],
                platform=value["platform"],
            )
        self.assertEqual(selected.name, attachment.name)

    def test_apple_attachment_extractor_rejects_duplicate_identity(self) -> None:
        value = self.endpoint_result()
        value["platform"] = "ios"
        value["driver"] = "direct_ffi"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("first", "second"):
                (root / name).write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaises(apple_matrix_evidence.EvidenceExtractionError):
                apple_matrix_evidence.find_endpoint_attachment(
                    root,
                    run_id=value["run_id"],
                    case_id=value["case_id"],
                    repetition=value["repetition"],
                    role=value["role"],
                    platform=value["platform"],
                )

    def test_endpoint_result_identity_mismatch_is_rejected(self) -> None:
        errors = matrix_contract.validate_endpoint_result(
            self.endpoint_result(),
            run_id="another-run",
            case_id="l1.physical.room.android-ios.single-file",
            repetition=1,
            role="sender",
            platform="android",
        )
        self.assert_error(errors, "run_id does not match the runner")

    def test_endpoint_result_hash_and_private_path_are_rejected(self) -> None:
        value = self.endpoint_result()
        value["source_summary"]["entries"][0]["sha256"] = "not-a-digest"
        value["device_model"] = "/data/user/0/private-model"
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "sha256 must be a lowercase SHA-256 digest")
        self.assert_error(errors, "contains sensitive absolute private path")

    def test_endpoint_result_requires_role_appropriate_summary(self) -> None:
        value = self.endpoint_result()
        value["source_summary"] = None
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "sender requires source_summary")

    def test_failed_endpoint_result_requires_typed_failure(self) -> None:
        value = self.endpoint_result()
        value["terminal_state"] = "failed"
        value["ordered_phases"][-1] = "failed"
        value["delivery_proof"] = False
        errors = matrix_contract.validate_endpoint_result(value)
        self.assert_error(errors, "failed endpoint requires failure")

    def test_typed_setup_failure_can_precede_a_summary(self) -> None:
        value = self.endpoint_result()
        value["terminal_state"] = "failed"
        value["ordered_phases"] = ["failed"]
        value["source_summary"] = None
        value["delivery_proof"] = False
        value["failure"] = {
            "code": "room_not_found",
            "phase": "setup",
            "recovery_action": "retry",
        }
        self.assertEqual(matrix_contract.validate_endpoint_result(value), [])

    def test_endpoint_result_cli_validates_identity_and_retains_normalized_json(
        self,
    ) -> None:
        contract = REPO_ROOT / "scripts" / "matrix_contract.py"
        value = self.endpoint_result()
        with tempfile.TemporaryDirectory() as directory:
            input_path = Path(directory) / "raw.json"
            output_path = Path(directory) / "sender.json"
            input_path.write_text(json.dumps(value), encoding="utf-8")
            subprocess.run(
                [
                    sys.executable,
                    str(contract),
                    "validate-endpoint-result",
                    str(input_path),
                    "--run-id",
                    value["run_id"],
                    "--case",
                    value["case_id"],
                    "--repetition",
                    str(value["repetition"]),
                    "--role",
                    value["role"],
                    "--platform",
                    value["platform"],
                    "--output",
                    str(output_path),
                ],
                cwd=REPO_ROOT,
                check=True,
                capture_output=True,
                text=True,
            )
            retained = json.loads(output_path.read_text(encoding="utf-8"))
        self.assertEqual(retained, value)

    def test_execution_record_retains_endpoint_artifact_paths(self) -> None:
        path = "cases/l1.physical.room.android-ios.single-file/r1/sender.json"
        record = matrix_contract.execution_record(
            run_id="fixture-run",
            case_id="l1.physical.room.android-ios.single-file",
            repetition=1,
            status="pass",
            endpoint_results=[path],
        )
        self.assertEqual(record["endpoint_results"], [path])

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
