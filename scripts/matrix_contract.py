#!/usr/bin/env python3
"""Validate and inspect the versioned end-to-end test matrix contract."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Iterable, Sequence


SCHEMA_VERSION = 1
RUN_SCHEMA_VERSION = 1
MAX_CASES = 512
MAX_PROFILES = 64
MAX_TEXT_LENGTH = 400
MAX_TIMEOUT_SECONDS = 86_400
MAX_TRANSFER_BYTES = 1 << 40
MAX_ENDPOINT_ENTRIES = 4096
MAX_ENDPOINT_PHASES = 64

IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9_.-]{0,95}$")
RUN_IDENTIFIER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,95}$")
PROFILE_IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")
REVISION = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SENSITIVE_TEXT_PATTERNS = {
    "Room Code-shaped secret": re.compile(
        r"(?i)(?<![a-z0-9])\d{6}-[a-z0-9]{4,8}-[a-z0-9]{4,8}(?![a-z0-9])"
    ),
    "invitation URI": re.compile(r"(?i)envoix://invite/v2/"),
    "absolute private path": re.compile(
        r"(?:/Users/|/home/|/data/user/|/private/|/tmp/|/var/folders/|/storage/emulated/)"
    ),
    "device serial canary": re.compile(r"ANDROID_SERIAL_CANARY"),
    "IPv4 address": re.compile(r"(?<![0-9.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9.])"),
}
REDACTIONS = (
    (
        re.compile(r"(?i)(?<![a-z0-9])\d{6}-[a-z0-9]{4,8}-[a-z0-9]{4,8}(?![a-z0-9])"),
        "[REDACTED_ROOM_CODE]",
    ),
    (
        re.compile(r"(?i)envoix://invite/v2/[^\s\"']+"),
        "[REDACTED_INVITATION]",
    ),
    (
        re.compile(r"(?i)\b((?:android_|device_)?serial|token|credential)=([^\s\"']+)"),
        r"\1=[REDACTED]",
    ),
    (re.compile(r"ANDROID_SERIAL_CANARY"), "[REDACTED_DEVICE_SERIAL]"),
    (
        re.compile(
            r"(?:/Users/|/home/|/data/user/|/private/|/tmp/|/var/folders/|"
            r"/storage/emulated/)[^\s\"']*"
        ),
        "[REDACTED_PATH]",
    ),
    (
        re.compile(r"(?<![0-9.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9.])"),
        "[REDACTED_NETWORK_ADDRESS]",
    ),
)

TOP_LEVEL_KEYS = {"schema_version", "registry_revision", "profiles", "cases"}
PROFILE_KEYS = {"kind", "description", "scenario", "size_bytes"}
REQUIRED_PROFILE_KEYS = {"kind", "description", "scenario"}
CASE_KEYS = {
    "case_id",
    "title",
    "owning_issues",
    "test_layer",
    "gate",
    "sender",
    "receiver",
    "transfer_profile",
    "invitation_input",
    "requested_path_policy",
    "fault_profile",
    "expected_terminal_state",
    "required_evidence",
    "support_status",
    "support_reason",
    "hardware_requirements",
    "build_variant",
    "required_repetitions",
    "timeout_seconds",
    "tags",
}

PROFILE_KINDS = {
    "single_file",
    "multiple_files",
    "multiple_folders",
    "multi_root",
}
TEST_LAYERS = {"l0_core", "l1_native", "l2_physical", "l3_extended"}
ENDPOINTS = {"rust_loopback", "cli", "android", "ios", "macos"}
INVITATION_INPUTS = {
    "room_code",
    "qr_deep_link",
    "manual",
    "mdns",
    "ble",
    "nfc",
    "trusted_exchange",
    "wifi_aware",
}
FUTURE_INVITATION_INPUTS = {"ble", "nfc", "trusted_exchange", "wifi_aware"}
PATH_POLICIES = {
    "auto",
    "direct_only",
    "relay_only",
    "direct_then_relay",
    "serverless_lan",
}
FAULT_PROFILES = {
    "none",
    "network_interrupt",
    "peer_departure",
    "sender_process_kill",
    "receiver_process_kill",
    "local_cancel",
    "peer_cancel",
    "cancel_message_loss",
    "wrong_secret",
    "wrong_identity",
    "malformed_manifest",
    "permission_revoked",
    "destination_unavailable",
    "quota_exhausted",
    "corruption",
    "direct_path_failure",
}
TERMINAL_STATES = {
    "completed",
    "paused_recoverable",
    "canceled",
    "failed",
}
EVIDENCE_FIELDS = {
    "activity_id",
    "attempt_count",
    "cleanup",
    "delivery_proof",
    "failure_code",
    "failure_phase",
    "final_tree",
    "log_redaction",
    "no_staging",
    "ordered_phases",
    "path_reason",
    "plaintext_bytes",
    "publication",
    "resume_reused_bytes",
    "selected_path",
    "sha256",
}
SUPPORT_STATUSES = {
    "required",
    "supported",
    "experimental",
    "planned",
    "hardware_blocked",
    "unsupported",
}
EXECUTION_STATUSES = {
    "pass",
    "product_failure",
    "infrastructure_failure",
    "not_run",
    "unsupported",
    "hardware_blocked",
}
HARDWARE_REQUIREMENTS = {
    "physical_android",
    "physical_ios",
    "physical_macos",
    "same_lan",
    "relay_service",
    "network_control",
    "ble_android",
    "ble_ios",
    "wifi_aware_android",
    "wifi_aware_ios",
}
BUILD_VARIANTS = {"debug", "release_equivalent"}
PROHIBITED_KEYS = {
    "absolute_path",
    "credential",
    "device_serial",
    "invite",
    "invitation_payload",
    "room_code",
    "token",
}
ENDPOINT_RESULT_KEYS = {
    "schema_version",
    "run_id",
    "case_id",
    "repetition",
    "role",
    "platform",
    "test_layer",
    "driver",
    "build_variant",
    "app_version",
    "core_version",
    "protocol_version",
    "device_model",
    "os_version",
    "capabilities",
    "activity_id",
    "job_id",
    "started_at",
    "finished_at",
    "terminal_state",
    "ordered_phases",
    "attempt_count",
    "selected_path",
    "path_reason",
    "source_summary",
    "destination_summary",
    "delivery_proof",
    "failure",
    "cleanup",
    "metrics",
}
ENDPOINT_ROLES = {"sender", "receiver"}
ENDPOINT_DRIVERS = {"direct_ffi", "product_activity"}
ENDPOINT_PHASES = {
    "waiting_for_peer",
    "pairing",
    "connecting",
    "offer",
    "transferring",
    "verifying",
    "saving",
    "waiting_for_receiver_save",
    "finalizing_delivery",
    "completed",
    "failed",
}
PATH_KINDS = {"direct", "relay", "wifi_aware", "other"}
ENTRY_KINDS = {"file", "directory"}
ENTRY_DISPOSITIONS = {"completed", "skipped", "renamed", "rejected", "failed"}
PUBLICATION_MECHANISMS = {
    "files_directory",
    "media_store",
    "mixed",
    "storage_access_framework",
    "test_local_directory",
}
RECOVERY_ACTIONS = {
    "none",
    "retry",
    "resume",
    "re_pair",
    "open_settings",
    "choose_folder",
}
FAILURE_PHASES = ENDPOINT_PHASES | {
    "setup",
    "authenticating",
    "negotiating",
    "committing",
    "driver_validation",
    "cleanup",
}
ENDPOINT_SUMMARY_KEYS = {
    "root_count",
    "file_count",
    "directory_count",
    "plaintext_bytes",
    "manifest_digest",
    "tree_digest",
    "entries",
    "publication",
}
ENDPOINT_ENTRY_KEYS = {
    "relative_path",
    "kind",
    "plaintext_bytes",
    "sha256",
    "disposition",
}


def _is_integer(value: object) -> bool:
    return type(value) is int


def _scan_prohibited_keys(value: object, context: str, errors: list[str]) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if isinstance(key, str) and key.lower() in PROHIBITED_KEYS:
                errors.append(f"{context} contains prohibited key {key!r}")
            _scan_prohibited_keys(child, f"{context}.{key}", errors)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _scan_prohibited_keys(child, f"{context}[{index}]", errors)


def _scan_sensitive_text(value: object, context: str, errors: list[str]) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            _scan_sensitive_text(child, f"{context}.{key}", errors)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _scan_sensitive_text(child, f"{context}[{index}]", errors)
    elif isinstance(value, str):
        for label, pattern in SENSITIVE_TEXT_PATTERNS.items():
            if pattern.search(value):
                errors.append(f"{context} contains sensitive {label}")


def _check_keys(
    value: dict[str, object],
    allowed: set[str],
    context: str,
    errors: list[str],
) -> None:
    unknown = sorted(set(value) - allowed)
    missing = sorted(allowed - set(value))
    if unknown:
        errors.append(f"{context} has unknown field(s): {', '.join(unknown)}")
    if missing:
        errors.append(f"{context} is missing field(s): {', '.join(missing)}")


def _check_text(
    value: object,
    context: str,
    errors: list[str],
    *,
    allow_empty: bool = False,
) -> str | None:
    if not isinstance(value, str):
        errors.append(f"{context} must be a string")
        return None
    if not allow_empty and not value:
        errors.append(f"{context} must not be empty")
    if len(value) > MAX_TEXT_LENGTH:
        errors.append(f"{context} exceeds {MAX_TEXT_LENGTH} characters")
    if any(ord(character) < 32 for character in value):
        errors.append(f"{context} contains a control character")
    return value


def _check_enum(
    value: object,
    allowed: set[str],
    context: str,
    errors: list[str],
) -> str | None:
    checked = _check_text(value, context, errors)
    if checked is not None and checked not in allowed:
        errors.append(f"{context} has unsupported value {checked!r}")
    return checked


def _check_string_list(
    value: object,
    allowed: set[str] | None,
    context: str,
    errors: list[str],
) -> list[str]:
    if not isinstance(value, list):
        errors.append(f"{context} must be a list")
        return []
    checked: list[str] = []
    for index, item in enumerate(value):
        text = _check_text(item, f"{context}[{index}]", errors)
        if text is None:
            continue
        if allowed is not None and text not in allowed:
            errors.append(f"{context}[{index}] has unsupported value {text!r}")
        checked.append(text)
    if len(checked) != len(set(checked)):
        errors.append(f"{context} contains duplicate values")
    return checked


def _check_nullable_text(
    value: object,
    context: str,
    errors: list[str],
) -> str | None:
    if value is None:
        return None
    return _check_text(value, context, errors)


def _check_digest(
    value: object,
    context: str,
    errors: list[str],
    *,
    nullable: bool = False,
) -> str | None:
    if nullable and value is None:
        return None
    digest = _check_text(value, context, errors)
    if digest is not None and not SHA256.fullmatch(digest):
        errors.append(f"{context} must be a lowercase SHA-256 digest")
    return digest


def _check_relative_path(
    value: object,
    context: str,
    errors: list[str],
) -> str | None:
    path = _check_text(value, context, errors)
    if path is None:
        return None
    parts = path.split("/")
    if (
        path.startswith("/")
        or "\\" in path
        or any(part in {"", ".", ".."} for part in parts)
    ):
        errors.append(f"{context} must be a normalized relative path")
    return path


def _validate_endpoint_summary(
    value: object,
    context: str,
    errors: list[str],
) -> None:
    if not isinstance(value, dict):
        errors.append(f"{context} must be an object")
        return
    _check_keys(value, ENDPOINT_SUMMARY_KEYS, context, errors)
    counts: dict[str, int] = {}
    for field in ("root_count", "file_count", "directory_count", "plaintext_bytes"):
        raw = value.get(field)
        maximum = (
            MAX_TRANSFER_BYTES if field == "plaintext_bytes" else MAX_ENDPOINT_ENTRIES
        )
        if not _is_integer(raw) or not 0 <= raw <= maximum:
            errors.append(f"{context}.{field} must be between 0 and {maximum}")
        else:
            counts[field] = raw
    _check_digest(
        value.get("manifest_digest"),
        f"{context}.manifest_digest",
        errors,
        nullable=True,
    )
    _check_digest(value.get("tree_digest"), f"{context}.tree_digest", errors)

    entries = value.get("entries")
    checked_paths: list[str] = []
    checked_file_count = 0
    checked_directory_count = 0
    checked_bytes = 0
    if not isinstance(entries, list):
        errors.append(f"{context}.entries must be a list")
    else:
        if len(entries) > MAX_ENDPOINT_ENTRIES:
            errors.append(f"{context}.entries exceeds {MAX_ENDPOINT_ENTRIES} entries")
        for index, entry in enumerate(entries):
            entry_context = f"{context}.entries[{index}]"
            if not isinstance(entry, dict):
                errors.append(f"{entry_context} must be an object")
                continue
            _check_keys(entry, ENDPOINT_ENTRY_KEYS, entry_context, errors)
            relative_path = _check_relative_path(
                entry.get("relative_path"),
                f"{entry_context}.relative_path",
                errors,
            )
            if relative_path is not None:
                checked_paths.append(relative_path)
            kind = _check_enum(
                entry.get("kind"),
                ENTRY_KINDS,
                f"{entry_context}.kind",
                errors,
            )
            size = entry.get("plaintext_bytes")
            if not _is_integer(size) or not 0 <= size <= MAX_TRANSFER_BYTES:
                errors.append(
                    f"{entry_context}.plaintext_bytes must be between 0 and "
                    f"{MAX_TRANSFER_BYTES}"
                )
            if kind == "file":
                checked_file_count += 1
                if _is_integer(size):
                    checked_bytes += size
                _check_digest(entry.get("sha256"), f"{entry_context}.sha256", errors)
            elif kind == "directory":
                checked_directory_count += 1
                if size != 0:
                    errors.append(
                        f"{entry_context}.plaintext_bytes must be 0 for a directory"
                    )
                if entry.get("sha256") is not None:
                    errors.append(
                        f"{entry_context}.sha256 must be null for a directory"
                    )
            _check_enum(
                entry.get("disposition"),
                ENTRY_DISPOSITIONS,
                f"{entry_context}.disposition",
                errors,
            )
    if len(checked_paths) != len(set(checked_paths)):
        errors.append(f"{context}.entries contains duplicate relative paths")
    if checked_paths != sorted(checked_paths):
        errors.append(f"{context}.entries must be sorted by relative_path")
    if counts.get("file_count") != checked_file_count:
        errors.append(f"{context}.file_count does not match entries")
    if counts.get("directory_count") != checked_directory_count:
        errors.append(f"{context}.directory_count does not match entries")
    if counts.get("plaintext_bytes") != checked_bytes:
        errors.append(f"{context}.plaintext_bytes does not match file entries")

    publication = value.get("publication")
    if publication is not None:
        publication_context = f"{context}.publication"
        if not isinstance(publication, dict):
            errors.append(f"{publication_context} must be an object or null")
        else:
            _check_keys(
                publication,
                {"mechanism", "committed"},
                publication_context,
                errors,
            )
            _check_enum(
                publication.get("mechanism"),
                PUBLICATION_MECHANISMS,
                f"{publication_context}.mechanism",
                errors,
            )
            if type(publication.get("committed")) is not bool:
                errors.append(f"{publication_context}.committed must be a boolean")


def validate_endpoint_result(
    value: object,
    *,
    run_id: str | None = None,
    case_id: str | None = None,
    repetition: int | None = None,
    role: str | None = None,
    platform: str | None = None,
) -> list[str]:
    """Return every contract violation in one sanitized endpoint result."""

    errors: list[str] = []
    _scan_prohibited_keys(value, "endpoint result", errors)
    _scan_sensitive_text(value, "endpoint result", errors)
    if not isinstance(value, dict):
        return errors + ["endpoint result must be an object"]
    _check_keys(value, ENDPOINT_RESULT_KEYS, "endpoint result", errors)
    if value.get("schema_version") != RUN_SCHEMA_VERSION:
        errors.append(f"endpoint result schema_version must be {RUN_SCHEMA_VERSION}")

    actual_run_id = value.get("run_id")
    if not isinstance(actual_run_id, str) or not RUN_IDENTIFIER.fullmatch(
        actual_run_id
    ):
        errors.append("endpoint result.run_id must be a stable identifier")
    if run_id is not None and actual_run_id != run_id:
        errors.append("endpoint result run_id does not match the runner")
    actual_case_id = value.get("case_id")
    if not isinstance(actual_case_id, str) or not IDENTIFIER.fullmatch(actual_case_id):
        errors.append("endpoint result.case_id must be a stable lowercase identifier")
    if case_id is not None and actual_case_id != case_id:
        errors.append("endpoint result case_id does not match the runner")
    actual_repetition = value.get("repetition")
    if not _is_integer(actual_repetition) or not 1 <= actual_repetition <= 10:
        errors.append("endpoint result.repetition must be between 1 and 10")
    if repetition is not None and actual_repetition != repetition:
        errors.append("endpoint result repetition does not match the runner")
    actual_role = _check_enum(
        value.get("role"),
        ENDPOINT_ROLES,
        "endpoint result.role",
        errors,
    )
    if role is not None and actual_role != role:
        errors.append("endpoint result role does not match the runner")
    actual_platform = _check_enum(
        value.get("platform"),
        ENDPOINTS - {"rust_loopback", "cli"},
        "endpoint result.platform",
        errors,
    )
    if platform is not None and actual_platform != platform:
        errors.append("endpoint result platform does not match the runner")
    actual_layer = _check_enum(
        value.get("test_layer"),
        TEST_LAYERS,
        "endpoint result.test_layer",
        errors,
    )
    actual_driver = _check_enum(
        value.get("driver"),
        ENDPOINT_DRIVERS,
        "endpoint result.driver",
        errors,
    )
    if actual_driver == "direct_ffi" and (
        actual_platform not in {"android", "ios", "macos"}
        or actual_layer != "l1_native"
    ):
        errors.append("direct_ffi endpoint results must be Android or Apple L1 evidence")
    if actual_driver == "product_activity" and actual_layer != "l2_physical":
        errors.append("product_activity endpoint results must be L2 evidence")
    actual_build_variant = _check_enum(
        value.get("build_variant"),
        BUILD_VARIANTS,
        "endpoint result.build_variant",
        errors,
    )
    if (
        actual_driver == "product_activity"
        and actual_build_variant != "release_equivalent"
    ):
        errors.append(
            "product_activity endpoint results require a release-equivalent build"
        )
    _check_text(value.get("app_version"), "endpoint result.app_version", errors)
    _check_nullable_text(
        value.get("core_version"),
        "endpoint result.core_version",
        errors,
    )
    protocol_version = value.get("protocol_version")
    if not _is_integer(protocol_version) or protocol_version <= 0:
        errors.append("endpoint result.protocol_version must be a positive integer")
    _check_text(value.get("device_model"), "endpoint result.device_model", errors)
    _check_text(value.get("os_version"), "endpoint result.os_version", errors)
    capabilities = _check_string_list(
        value.get("capabilities"),
        None,
        "endpoint result.capabilities",
        errors,
    )
    if len(capabilities) > 32:
        errors.append("endpoint result.capabilities exceeds 32 entries")
    for index, capability in enumerate(capabilities):
        if not PROFILE_IDENTIFIER.fullmatch(capability):
            errors.append(
                f"endpoint result.capabilities[{index}] must be a stable "
                "lowercase identifier"
            )
    identifiers: dict[str, str | None] = {}
    for field in ("activity_id", "job_id"):
        identifier = _check_nullable_text(
            value.get(field),
            f"endpoint result.{field}",
            errors,
        )
        identifiers[field] = identifier
        if identifier is not None and not RUN_IDENTIFIER.fullmatch(identifier):
            errors.append(
                f"endpoint result.{field} must be a stable identifier or null"
            )
    if actual_driver == "product_activity" and identifiers["activity_id"] is None:
        errors.append("product_activity endpoint results require activity_id")

    started_at = value.get("started_at")
    finished_at = value.get("finished_at")
    for field, timestamp in (("started_at", started_at), ("finished_at", finished_at)):
        if not _is_integer(timestamp) or timestamp < 0:
            errors.append(f"endpoint result.{field} must be a non-negative integer")
    if (
        _is_integer(started_at)
        and _is_integer(finished_at)
        and finished_at < started_at
    ):
        errors.append("endpoint result.finished_at must not precede started_at")

    terminal_state = _check_enum(
        value.get("terminal_state"),
        {"completed", "failed"},
        "endpoint result.terminal_state",
        errors,
    )
    raw_phases = value.get("ordered_phases")
    phases: list[str] = []
    if not isinstance(raw_phases, list):
        errors.append("endpoint result.ordered_phases must be a list")
    else:
        for index, item in enumerate(raw_phases):
            phase = _check_enum(
                item,
                ENDPOINT_PHASES,
                f"endpoint result.ordered_phases[{index}]",
                errors,
            )
            if phase is not None:
                phases.append(phase)
    if not phases:
        errors.append("endpoint result.ordered_phases must not be empty")
    if len(phases) > MAX_ENDPOINT_PHASES:
        errors.append(
            f"endpoint result.ordered_phases exceeds {MAX_ENDPOINT_PHASES} entries"
        )
    if terminal_state is not None and phases and phases[-1] != terminal_state:
        errors.append("endpoint result.ordered_phases must end at terminal_state")
    attempt_count = value.get("attempt_count")
    if not _is_integer(attempt_count) or not 1 <= attempt_count <= 100:
        errors.append("endpoint result.attempt_count must be between 1 and 100")
    selected_path = value.get("selected_path")
    if selected_path is not None:
        _check_enum(
            selected_path,
            PATH_KINDS,
            "endpoint result.selected_path",
            errors,
        )
    if (
        actual_driver == "product_activity"
        and terminal_state == "completed"
        and selected_path is None
    ):
        errors.append("product_activity endpoint results require selected_path")
    path_reason = _check_nullable_text(
        value.get("path_reason"),
        "endpoint result.path_reason",
        errors,
    )
    if path_reason is not None and not PROFILE_IDENTIFIER.fullmatch(path_reason):
        errors.append(
            "endpoint result.path_reason must be a stable lowercase identifier"
        )

    source = value.get("source_summary")
    destination = value.get("destination_summary")
    if actual_role == "sender":
        if source is None and terminal_state == "completed":
            errors.append("sender requires source_summary")
        elif source is not None:
            _validate_endpoint_summary(source, "endpoint result.source_summary", errors)
        if destination is not None:
            errors.append("sender destination_summary must be null")
    elif actual_role == "receiver":
        if source is not None:
            errors.append("receiver source_summary must be null")
        if destination is None and terminal_state == "completed":
            errors.append("receiver requires destination_summary")
        elif destination is not None:
            _validate_endpoint_summary(
                destination,
                "endpoint result.destination_summary",
                errors,
            )
        if (
            actual_driver == "product_activity"
            and terminal_state == "completed"
            and isinstance(destination, dict)
        ):
            publication = destination.get("publication")
            if not isinstance(publication, dict):
                errors.append(
                    "completed product_activity receiver requires publication"
                )
            else:
                if publication.get("mechanism") == "test_local_directory":
                    errors.append(
                        "product_activity receiver cannot use test-local publication"
                    )
                if publication.get("committed") is not True:
                    errors.append(
                        "completed product_activity receiver requires committed publication"
                    )

    delivery_proof = value.get("delivery_proof")
    if type(delivery_proof) is not bool:
        errors.append("endpoint result.delivery_proof must be a boolean")
    failure = value.get("failure")
    if terminal_state == "completed":
        if failure is not None:
            errors.append("completed endpoint failure must be null")
        if delivery_proof is not True:
            errors.append("completed endpoint requires delivery_proof")
    elif terminal_state == "failed":
        if not isinstance(failure, dict):
            errors.append("failed endpoint requires failure")
        else:
            _check_keys(
                failure,
                {"code", "phase", "recovery_action"},
                "endpoint result.failure",
                errors,
            )
            code = _check_text(
                failure.get("code"),
                "endpoint result.failure.code",
                errors,
            )
            if code is not None and not PROFILE_IDENTIFIER.fullmatch(code):
                errors.append(
                    "endpoint result.failure.code must be a stable lowercase identifier"
                )
            _check_enum(
                failure.get("phase"),
                FAILURE_PHASES,
                "endpoint result.failure.phase",
                errors,
            )
            _check_enum(
                failure.get("recovery_action"),
                RECOVERY_ACTIONS,
                "endpoint result.failure.recovery_action",
                errors,
            )
        if delivery_proof is not False:
            errors.append("failed endpoint must not claim delivery_proof")

    cleanup = value.get("cleanup")
    if not isinstance(cleanup, dict):
        errors.append("endpoint result.cleanup must be an object")
    else:
        _check_keys(
            cleanup,
            {"test_owned", "completed"},
            "endpoint result.cleanup",
            errors,
        )
        if cleanup.get("test_owned") is not True:
            errors.append("endpoint result.cleanup.test_owned must be true")
        if type(cleanup.get("completed")) is not bool:
            errors.append("endpoint result.cleanup.completed must be a boolean")
        if terminal_state == "completed" and cleanup.get("completed") is not True:
            errors.append("completed endpoint requires completed cleanup")

    metrics = value.get("metrics")
    if not isinstance(metrics, dict):
        errors.append("endpoint result.metrics must be an object")
    else:
        _check_keys(
            metrics,
            {"plaintext_bytes", "elapsed_ms"},
            "endpoint result.metrics",
            errors,
        )
        metric_bytes = metrics.get("plaintext_bytes")
        if not _is_integer(metric_bytes) or not 0 <= metric_bytes <= MAX_TRANSFER_BYTES:
            errors.append(
                "endpoint result.metrics.plaintext_bytes must be a bounded integer"
            )
        elapsed_ms = metrics.get("elapsed_ms")
        if not _is_integer(elapsed_ms) or elapsed_ms < 0:
            errors.append(
                "endpoint result.metrics.elapsed_ms must be a non-negative integer"
            )
        if (
            _is_integer(started_at)
            and _is_integer(finished_at)
            and _is_integer(elapsed_ms)
            and elapsed_ms != finished_at - started_at
        ):
            errors.append(
                "endpoint result.metrics.elapsed_ms does not match endpoint timestamps"
            )
        summary = source if actual_role == "sender" else destination
        if (
            isinstance(summary, dict)
            and _is_integer(metric_bytes)
            and metric_bytes != summary.get("plaintext_bytes")
        ):
            errors.append(
                "endpoint result.metrics.plaintext_bytes does not match endpoint summary"
            )
    return errors


def read_endpoint_result(
    path: Path,
    **expected: object,
) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ValueError(f"could not read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"could not parse {path}: {error}") from error
    errors = validate_endpoint_result(value, **expected)
    if errors:
        formatted = "\n".join(f"- {error}" for error in errors)
        raise ValueError(f"{path} violates the endpoint-result contract:\n{formatted}")
    return value


def _validate_profile(
    profile_id: object,
    value: object,
    errors: list[str],
) -> None:
    context = f"profiles.{profile_id}"
    if not isinstance(profile_id, str) or not PROFILE_IDENTIFIER.fullmatch(profile_id):
        errors.append(f"profiles has invalid identifier {profile_id!r}")
    if not isinstance(value, dict):
        errors.append(f"{context} must be an object")
        return
    unknown = sorted(set(value) - PROFILE_KEYS)
    missing = sorted(REQUIRED_PROFILE_KEYS - set(value))
    if unknown:
        errors.append(f"{context} has unknown field(s): {', '.join(unknown)}")
    if missing:
        errors.append(f"{context} is missing field(s): {', '.join(missing)}")
    _check_enum(value.get("kind"), PROFILE_KINDS, f"{context}.kind", errors)
    _check_text(value.get("description"), f"{context}.description", errors)
    scenario = _check_text(value.get("scenario"), f"{context}.scenario", errors)
    if scenario is not None and not PROFILE_IDENTIFIER.fullmatch(scenario):
        errors.append(f"{context}.scenario must be a stable lowercase identifier")
    size_bytes = value.get("size_bytes")
    if size_bytes is not None:
        if not _is_integer(size_bytes) or not 0 <= size_bytes <= MAX_TRANSFER_BYTES:
            errors.append(
                f"{context}.size_bytes must be between 0 and {MAX_TRANSFER_BYTES}"
            )


def _validate_issue_list(value: object, context: str, errors: list[str]) -> None:
    if not isinstance(value, list) or not value:
        errors.append(f"{context} must be a non-empty list")
        return
    if len(value) > 16:
        errors.append(f"{context} exceeds 16 entries")
    checked: list[int] = []
    for index, issue in enumerate(value):
        if not _is_integer(issue) or issue <= 0:
            errors.append(f"{context}[{index}] must be a positive issue number")
        else:
            checked.append(issue)
    if len(checked) != len(set(checked)):
        errors.append(f"{context} contains duplicate issue numbers")


def _validate_case(
    value: object,
    index: int,
    profile_ids: set[str],
    errors: list[str],
) -> str | None:
    context = f"cases[{index}]"
    if not isinstance(value, dict):
        errors.append(f"{context} must be an object")
        return None
    _check_keys(value, CASE_KEYS, context, errors)

    case_id = _check_text(value.get("case_id"), f"{context}.case_id", errors)
    if case_id is not None and not IDENTIFIER.fullmatch(case_id):
        errors.append(f"{context}.case_id must be a stable lowercase identifier")
    _check_text(value.get("title"), f"{context}.title", errors)
    _validate_issue_list(value.get("owning_issues"), f"{context}.owning_issues", errors)
    layer = _check_enum(
        value.get("test_layer"), TEST_LAYERS, f"{context}.test_layer", errors
    )
    gate = _check_text(value.get("gate"), f"{context}.gate", errors)
    if gate is not None and not IDENTIFIER.fullmatch(gate):
        errors.append(f"{context}.gate must be a stable lowercase identifier")
    _check_enum(value.get("sender"), ENDPOINTS, f"{context}.sender", errors)
    _check_enum(value.get("receiver"), ENDPOINTS, f"{context}.receiver", errors)
    profile = _check_text(
        value.get("transfer_profile"),
        f"{context}.transfer_profile",
        errors,
    )
    if profile is not None and profile not in profile_ids:
        errors.append(
            f"{context}.transfer_profile references unknown profile {profile!r}"
        )
    invitation_input = _check_enum(
        value.get("invitation_input"),
        INVITATION_INPUTS,
        f"{context}.invitation_input",
        errors,
    )
    path_policy = _check_enum(
        value.get("requested_path_policy"),
        PATH_POLICIES,
        f"{context}.requested_path_policy",
        errors,
    )
    fault_profile = _check_enum(
        value.get("fault_profile"),
        FAULT_PROFILES,
        f"{context}.fault_profile",
        errors,
    )
    terminal_state = _check_enum(
        value.get("expected_terminal_state"),
        TERMINAL_STATES,
        f"{context}.expected_terminal_state",
        errors,
    )
    evidence = _check_string_list(
        value.get("required_evidence"),
        EVIDENCE_FIELDS,
        f"{context}.required_evidence",
        errors,
    )
    if not evidence:
        errors.append(f"{context}.required_evidence must not be empty")
    support_status = _check_enum(
        value.get("support_status"),
        SUPPORT_STATUSES,
        f"{context}.support_status",
        errors,
    )
    support_reason = value.get("support_reason")
    if support_status in {"required", "supported"}:
        if support_reason is not None:
            errors.append(
                f"{context}.support_reason must be null for {support_status} cases"
            )
    else:
        _check_text(support_reason, f"{context}.support_reason", errors)
    hardware = _check_string_list(
        value.get("hardware_requirements"),
        HARDWARE_REQUIREMENTS,
        f"{context}.hardware_requirements",
        errors,
    )
    build_variant = _check_enum(
        value.get("build_variant"),
        BUILD_VARIANTS,
        f"{context}.build_variant",
        errors,
    )
    repetitions = value.get("required_repetitions")
    if not _is_integer(repetitions) or not 1 <= repetitions <= 10:
        errors.append(f"{context}.required_repetitions must be between 1 and 10")
    timeout = value.get("timeout_seconds")
    if not _is_integer(timeout) or not 30 <= timeout <= MAX_TIMEOUT_SECONDS:
        errors.append(
            f"{context}.timeout_seconds must be between 30 and {MAX_TIMEOUT_SECONDS}"
        )
    tags = _check_string_list(value.get("tags"), None, f"{context}.tags", errors)
    for tag_index, tag in enumerate(tags):
        if not PROFILE_IDENTIFIER.fullmatch(tag):
            errors.append(f"{context}.tags[{tag_index}] must be a lowercase identifier")

    if support_status == "hardware_blocked" and not hardware:
        errors.append(f"{context} is hardware_blocked but has no hardware requirements")
    if invitation_input in FUTURE_INVITATION_INPUTS and support_status in {
        "required",
        "supported",
    }:
        errors.append(
            f"{context} cannot be {support_status} while {invitation_input} is a future carrier"
        )
    if layer == "l2_physical":
        if build_variant != "release_equivalent":
            errors.append(f"{context} L2 cases must use a release-equivalent build")
        if not _is_integer(repetitions) or repetitions < 3:
            errors.append(f"{context} L2 cases require at least three repetitions")
        required_l2 = {"activity_id", "log_redaction", "ordered_phases"}
        missing = sorted(required_l2 - set(evidence))
        if missing:
            errors.append(f"{context} L2 evidence is missing: {', '.join(missing)}")
    if layer == "l2_physical" and terminal_state == "completed":
        required_completion = {
            "attempt_count",
            "cleanup",
            "delivery_proof",
            "final_tree",
            "plaintext_bytes",
            "publication",
            "selected_path",
            "sha256",
        }
        missing = sorted(required_completion - set(evidence))
        if missing:
            errors.append(
                f"{context} completed L2 evidence is missing: {', '.join(missing)}"
            )
    if path_policy == "direct_then_relay" and "path_reason" not in evidence:
        errors.append(f"{context} fallback cases require path_reason evidence")
    if fault_profile == "network_interrupt" and "resume_reused_bytes" not in evidence:
        errors.append(f"{context} recovery cases require resume_reused_bytes evidence")
    if terminal_state == "failed":
        missing = sorted({"failure_code", "failure_phase"} - set(evidence))
        if missing:
            errors.append(
                f"{context} failed-case evidence is missing: {', '.join(missing)}"
            )

    return case_id


def validate_registry(value: object) -> list[str]:
    """Return every contract violation found in a decoded registry."""

    errors: list[str] = []
    _scan_prohibited_keys(value, "registry", errors)
    _scan_sensitive_text(value, "registry", errors)
    if not isinstance(value, dict):
        return errors + ["registry must be an object"]
    _check_keys(value, TOP_LEVEL_KEYS, "registry", errors)
    if value.get("schema_version") != SCHEMA_VERSION:
        errors.append(f"registry.schema_version must be {SCHEMA_VERSION}")
    revision = _check_text(
        value.get("registry_revision"), "registry.registry_revision", errors
    )
    if revision is not None and not REVISION.fullmatch(revision):
        errors.append("registry.registry_revision must use YYYY-MM-DD")

    profiles = value.get("profiles")
    profile_ids: set[str] = set()
    if not isinstance(profiles, dict) or not profiles:
        errors.append("registry.profiles must be a non-empty object")
    else:
        if len(profiles) > MAX_PROFILES:
            errors.append(f"registry.profiles exceeds {MAX_PROFILES} entries")
        profile_ids = set(profiles)
        for profile_id, profile in profiles.items():
            _validate_profile(profile_id, profile, errors)

    cases = value.get("cases")
    if not isinstance(cases, list) or not cases:
        errors.append("registry.cases must be a non-empty list")
    else:
        if len(cases) > MAX_CASES:
            errors.append(f"registry.cases exceeds {MAX_CASES} entries")
        seen: set[str] = set()
        for index, case in enumerate(cases):
            case_id = _validate_case(case, index, profile_ids, errors)
            if case_id is not None:
                if case_id in seen:
                    errors.append(
                        f"registry.cases contains duplicate case_id {case_id!r}"
                    )
                seen.add(case_id)
    return errors


def read_registry(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ValueError(f"could not read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"could not parse {path}: {error}") from error
    errors = validate_registry(value)
    if errors:
        formatted = "\n".join(f"- {error}" for error in errors)
        raise ValueError(f"{path} violates the matrix contract:\n{formatted}")
    return value


def select_cases(
    registry: dict[str, Any],
    *,
    statuses: set[str] | None = None,
    layer: str | None = None,
    gate: str | None = None,
    tag: str | None = None,
) -> list[dict[str, Any]]:
    cases: Iterable[dict[str, Any]] = registry["cases"]
    if statuses:
        cases = (case for case in cases if case["support_status"] in statuses)
    if layer:
        cases = (case for case in cases if case["test_layer"] == layer)
    if gate:
        cases = (case for case in cases if case["gate"] == gate)
    if tag:
        cases = (case for case in cases if tag in case["tags"])
    return list(cases)


def resolve_cases(
    registry: dict[str, Any],
    *,
    case_ids: Sequence[str] = (),
    gate: str | None = None,
    tag: str | None = None,
    legacy_scenarios: Sequence[str] = (),
    legacy_directions: Sequence[str] = (),
) -> tuple[list[dict[str, Any]], list[str], str]:
    """Resolve exactly one explicit selection mode in registry order."""

    modes = sum(
        (
            bool(case_ids),
            gate is not None,
            tag is not None,
            bool(legacy_scenarios or legacy_directions),
        )
    )
    if modes != 1:
        raise ValueError("select exactly one of case IDs, gate, tag, or legacy inputs")

    warnings: list[str] = []
    selection = ""
    selected_ids: set[str]
    if case_ids:
        if len(case_ids) != len(set(case_ids)):
            raise ValueError("case selection contains duplicate case IDs")
        known_ids = {case["case_id"] for case in registry["cases"]}
        unknown = sorted(set(case_ids) - known_ids)
        if unknown:
            raise ValueError(f"unknown case ID(s): {', '.join(unknown)}")
        selected_ids = set(case_ids)
        selection = "case"
    elif gate is not None:
        selected_ids = {
            case["case_id"] for case in registry["cases"] if case["gate"] == gate
        }
        selection = f"gate:{gate}"
    elif tag is not None:
        selected_ids = {
            case["case_id"] for case in registry["cases"] if tag in case["tags"]
        }
        selection = f"tag:{tag}"
    else:
        scenarios = set(legacy_scenarios)
        directions = set(legacy_directions)
        if not scenarios or not directions:
            raise ValueError("legacy selection requires scenarios and directions")
        selected_ids = set()
        mapped_pairs: set[tuple[str, str]] = set()
        for case in registry["cases"]:
            if case["gate"] != "current-physical-harness":
                continue
            direction = f"{case['sender']}:{case['receiver']}"
            scenario = registry["profiles"][case["transfer_profile"]]["scenario"]
            if direction in directions and scenario in scenarios:
                selected_ids.add(case["case_id"])
                mapped_pairs.add((direction, scenario))
        for direction in sorted(directions):
            for scenario in sorted(scenarios):
                if (direction, scenario) not in mapped_pairs:
                    warnings.append(
                        f"legacy combination {direction}/{scenario} has no registry case"
                    )
        selection = "legacy"

    selected = [case for case in registry["cases"] if case["case_id"] in selected_ids]
    if not selected:
        raise ValueError(f"selection {selection!r} matched no registry cases")
    return selected, warnings, selection


def build_run_plan(
    registry: dict[str, Any],
    cases: Sequence[dict[str, Any]],
    *,
    run_id: str,
    tested_commit: str,
    build_variant: str,
    selection: str,
    dry_run: bool,
    repetitions: int | None = None,
) -> dict[str, Any]:
    if not isinstance(run_id, str) or not RUN_IDENTIFIER.fullmatch(run_id):
        raise ValueError("run ID must be at most 96 letters, digits, '.', '-' or '_'")
    if not re.fullmatch(r"[0-9a-f]{7,64}", tested_commit):
        raise ValueError("tested commit must be a 7-64 character lowercase Git SHA")
    if build_variant not in BUILD_VARIANTS:
        raise ValueError(f"unsupported build variant {build_variant!r}")
    if repetitions is not None and not 1 <= repetitions <= 10:
        raise ValueError("repetition override must be between 1 and 10")

    executions: list[dict[str, Any]] = []
    for case in cases:
        case_repetitions = case["required_repetitions"]
        if repetitions is not None:
            if repetitions < case_repetitions:
                raise ValueError(
                    f"{case['case_id']} requires at least {case_repetitions} repetitions"
                )
            case_repetitions = repetitions
        disposition = {
            "planned": "not_run",
            "unsupported": "unsupported",
            "hardware_blocked": "hardware_blocked",
        }.get(case["support_status"], "execute")
        if dry_run and disposition == "execute":
            disposition = "not_run"
        if disposition == "execute" and case["build_variant"] != build_variant:
            raise ValueError(
                f"{case['case_id']} requires build variant {case['build_variant']!r}"
            )
        scenario = registry["profiles"][case["transfer_profile"]]["scenario"]
        for repetition in range(1, case_repetitions + 1):
            executions.append(
                {
                    "case_id": case["case_id"],
                    "repetition": repetition,
                    "sender": case["sender"],
                    "receiver": case["receiver"],
                    "scenario": scenario,
                    "support_status": case["support_status"],
                    "timeout_seconds": case["timeout_seconds"],
                    "disposition": disposition,
                }
            )
    return {
        "schema_version": RUN_SCHEMA_VERSION,
        "registry_revision": registry["registry_revision"],
        "run_id": run_id,
        "tested_commit": tested_commit,
        "build_variant": build_variant,
        "selection": selection,
        "dry_run": dry_run,
        "executions": executions,
    }


def validate_run_plan(plan: object, registry: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if not isinstance(plan, dict):
        return ["run plan must be an object"]
    expected_keys = {
        "schema_version",
        "registry_revision",
        "run_id",
        "tested_commit",
        "build_variant",
        "selection",
        "dry_run",
        "executions",
    }
    _check_keys(plan, expected_keys, "run plan", errors)
    if plan.get("schema_version") != RUN_SCHEMA_VERSION:
        errors.append(f"run plan schema_version must be {RUN_SCHEMA_VERSION}")
    if plan.get("registry_revision") != registry["registry_revision"]:
        errors.append("run plan registry revision does not match the registry")
    run_id = plan.get("run_id")
    if not isinstance(run_id, str) or not RUN_IDENTIFIER.fullmatch(run_id):
        errors.append("run plan run_id must be a stable identifier")
    tested_commit = plan.get("tested_commit")
    if not isinstance(tested_commit, str) or not re.fullmatch(
        r"[0-9a-f]{7,64}", tested_commit
    ):
        errors.append("run plan tested_commit must be a lowercase Git SHA")
    if plan.get("build_variant") not in BUILD_VARIANTS:
        errors.append("run plan has an unsupported build_variant")
    _check_text(plan.get("selection"), "run plan selection", errors)
    if type(plan.get("dry_run")) is not bool:
        errors.append("run plan dry_run must be a boolean")

    known_cases = {case["case_id"]: case for case in registry["cases"]}
    executions = plan.get("executions")
    if not isinstance(executions, list) or not executions:
        errors.append("run plan executions must be a non-empty list")
        return errors
    expected_execution_keys = {
        "case_id",
        "repetition",
        "sender",
        "receiver",
        "scenario",
        "support_status",
        "timeout_seconds",
        "disposition",
    }
    identities: set[tuple[str, int]] = set()
    repetitions_by_case: dict[str, list[int]] = {}
    for index, execution in enumerate(executions):
        context = f"run plan executions[{index}]"
        if not isinstance(execution, dict):
            errors.append(f"{context} must be an object")
            continue
        _check_keys(execution, expected_execution_keys, context, errors)
        case_id = execution.get("case_id")
        case = known_cases.get(case_id)
        if case is None:
            errors.append(f"{context} references unknown case {case_id!r}")
            continue
        repetition = execution.get("repetition")
        if not _is_integer(repetition) or not 1 <= repetition <= 10:
            errors.append(f"{context}.repetition must be between 1 and 10")
            continue
        identity = (case_id, repetition)
        if identity in identities:
            errors.append(f"{context} duplicates {case_id} repetition {repetition}")
        identities.add(identity)
        repetitions_by_case.setdefault(case_id, []).append(repetition)
        if execution.get("sender") != case["sender"]:
            errors.append(f"{context}.sender does not match the registry")
        if execution.get("receiver") != case["receiver"]:
            errors.append(f"{context}.receiver does not match the registry")
        expected_scenario = registry["profiles"][case["transfer_profile"]]["scenario"]
        if execution.get("scenario") != expected_scenario:
            errors.append(f"{context}.scenario does not match the registry")
        if execution.get("support_status") != case["support_status"]:
            errors.append(f"{context}.support_status does not match the registry")
        if execution.get("timeout_seconds") != case["timeout_seconds"]:
            errors.append(f"{context}.timeout_seconds does not match the registry")
        disposition = execution.get("disposition")
        if disposition not in {
            "execute",
            "not_run",
            "unsupported",
            "hardware_blocked",
        }:
            errors.append(f"{context}.disposition is unsupported")
        expected_disposition = {
            "planned": "not_run",
            "unsupported": "unsupported",
            "hardware_blocked": "hardware_blocked",
        }.get(case["support_status"], "execute")
        if plan.get("dry_run") and expected_disposition == "execute":
            expected_disposition = "not_run"
        if disposition != expected_disposition:
            errors.append(f"{context}.disposition does not match support and run state")
        if plan.get("dry_run") and disposition == "execute":
            errors.append(f"{context} cannot execute in a dry-run")
        if (
            disposition == "execute"
            and plan.get("build_variant") != case["build_variant"]
        ):
            errors.append(f"{context} build variant does not match the registry")
    for case_id, repetitions in repetitions_by_case.items():
        case = known_cases[case_id]
        maximum = max(repetitions)
        if sorted(repetitions) != list(range(1, maximum + 1)):
            errors.append(f"run plan repetitions for {case_id} must be consecutive")
        if maximum < case["required_repetitions"]:
            errors.append(
                f"run plan has fewer than {case['required_repetitions']} "
                f"repetitions for {case_id}"
            )
    return errors


def read_run_plan(path: Path, registry: dict[str, Any]) -> dict[str, Any]:
    try:
        plan = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ValueError(f"could not read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"could not parse {path}: {error}") from error
    errors = validate_run_plan(plan, registry)
    if errors:
        formatted = "\n".join(f"- {error}" for error in errors)
        raise ValueError(f"{path} violates the run-plan contract:\n{formatted}")
    return plan


def execution_record(
    *,
    run_id: str,
    case_id: str,
    repetition: int,
    status: str,
    failure_code: str | None = None,
    sanitized_logs: Sequence[str] = (),
    endpoint_results: Sequence[str] = (),
) -> dict[str, Any]:
    if not isinstance(run_id, str) or not RUN_IDENTIFIER.fullmatch(run_id):
        raise ValueError("run ID must be a stable identifier")
    if not isinstance(case_id, str) or not IDENTIFIER.fullmatch(case_id):
        raise ValueError("case ID must be a stable lowercase identifier")
    if not _is_integer(repetition) or not 1 <= repetition <= 10:
        raise ValueError("repetition must be between 1 and 10")
    if status not in EXECUTION_STATUSES:
        raise ValueError(f"unsupported execution status {status!r}")
    classification = {
        "product_failure": "product",
        "infrastructure_failure": "infrastructure",
    }.get(status)
    if classification is None and failure_code is not None:
        raise ValueError(f"{status} cannot have a failure code")
    if classification is not None:
        if failure_code is None or not PROFILE_IDENTIFIER.fullmatch(failure_code):
            raise ValueError("failure status requires a stable lowercase failure code")
    for path in (*sanitized_logs, *endpoint_results):
        if not path or Path(path).is_absolute() or ".." in Path(path).parts:
            raise ValueError("artifact paths must be artifact-relative")
    return {
        "schema_version": RUN_SCHEMA_VERSION,
        "run_id": run_id,
        "case_id": case_id,
        "repetition": repetition,
        "execution_status": status,
        "classification": classification,
        "failure_code": failure_code,
        "sanitized_logs": list(sanitized_logs),
        "endpoint_results": list(endpoint_results),
    }


def validate_execution_record(
    record: object,
    execution: dict[str, Any],
) -> list[str]:
    errors: list[str] = []
    if not isinstance(record, dict):
        return ["execution record must be an object"]
    expected_keys = {
        "schema_version",
        "run_id",
        "case_id",
        "repetition",
        "execution_status",
        "classification",
        "failure_code",
        "sanitized_logs",
        "endpoint_results",
    }
    _check_keys(record, expected_keys, "execution record", errors)
    if record.get("schema_version") != RUN_SCHEMA_VERSION:
        errors.append(f"execution record schema_version must be {RUN_SCHEMA_VERSION}")
    if record.get("run_id") != execution["run_id"]:
        errors.append("execution record run_id does not match the plan")
    if record.get("case_id") != execution["case_id"]:
        errors.append("execution record case_id does not match the plan")
    if record.get("repetition") != execution["repetition"]:
        errors.append("execution record repetition does not match the plan")
    status = record.get("execution_status")
    if status not in EXECUTION_STATUSES:
        errors.append(f"execution record has unsupported status {status!r}")
    expected_classification = {
        "product_failure": "product",
        "infrastructure_failure": "infrastructure",
    }.get(status)
    if record.get("classification") != expected_classification:
        errors.append("execution record classification does not match its status")
    failure_code = record.get("failure_code")
    if expected_classification is None:
        if failure_code is not None:
            errors.append("non-failure execution record must not have a failure code")
    elif not isinstance(failure_code, str) or not PROFILE_IDENTIFIER.fullmatch(
        failure_code
    ):
        errors.append("failure execution record requires a stable failure code")
    logs = record.get("sanitized_logs")
    if not isinstance(logs, list):
        errors.append("execution record sanitized_logs must be a list")
    else:
        for path in logs:
            if (
                not isinstance(path, str)
                or Path(path).is_absolute()
                or ".." in Path(path).parts
            ):
                errors.append(
                    "execution record sanitized_logs must be artifact-relative"
                )
    endpoint_results = record.get("endpoint_results")
    if not isinstance(endpoint_results, list):
        errors.append("execution record endpoint_results must be a list")
    else:
        for path in endpoint_results:
            if (
                not isinstance(path, str)
                or Path(path).is_absolute()
                or ".." in Path(path).parts
            ):
                errors.append(
                    "execution record endpoint_results must be artifact-relative"
                )
    allowed_statuses = {
        "execute": {"pass", "product_failure", "infrastructure_failure"},
        "not_run": {"not_run"},
        "unsupported": {"unsupported"},
        "hardware_blocked": {"hardware_blocked"},
    }[execution["disposition"]]
    if status not in allowed_statuses:
        errors.append(
            f"execution status {status!r} is invalid for {execution['disposition']!r}"
        )
    return errors


def _record_path(records_root: Path, execution: dict[str, Any]) -> Path:
    return (
        records_root
        / execution["case_id"]
        / f"r{execution['repetition']}"
        / "result.json"
    )


def aggregate_run(
    registry: dict[str, Any],
    plan: dict[str, Any],
    records_root: Path,
) -> dict[str, Any]:
    results: list[dict[str, Any]] = []
    for planned_execution in plan["executions"]:
        execution = {**planned_execution, "run_id": plan["run_id"]}
        path = _record_path(records_root, execution)
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            record = execution_record(
                run_id=plan["run_id"],
                case_id=execution["case_id"],
                repetition=execution["repetition"],
                status="infrastructure_failure",
                failure_code="missing_or_malformed_execution_record",
            )
        errors = validate_execution_record(record, execution)
        if errors:
            record = execution_record(
                run_id=plan["run_id"],
                case_id=execution["case_id"],
                repetition=execution["repetition"],
                status="infrastructure_failure",
                failure_code="invalid_execution_record",
            )
        result = {
            **record,
            "support_status": execution["support_status"],
            "sender": execution["sender"],
            "receiver": execution["receiver"],
            "scenario": execution["scenario"],
        }
        results.append(result)

    counts = Counter(result["execution_status"] for result in results)
    required = [result for result in results if result["support_status"] == "required"]
    if not required:
        release_gate = "not_applicable"
    elif all(result["execution_status"] == "pass" for result in required):
        release_gate = "pass"
    else:
        release_gate = "fail"
    run_status = (
        "failure"
        if counts["product_failure"] or counts["infrastructure_failure"]
        else "complete"
    )
    return {
        "schema_version": RUN_SCHEMA_VERSION,
        "registry_revision": plan["registry_revision"],
        "run_id": plan["run_id"],
        "tested_commit": plan["tested_commit"],
        "build_variant": plan["build_variant"],
        "selection": plan["selection"],
        "dry_run": plan["dry_run"],
        "run_status": run_status,
        "release_gate": release_gate,
        "summary": {status: counts[status] for status in sorted(EXECUTION_STATUSES)},
        "results": results,
    }


def render_run_report(aggregate: dict[str, Any]) -> str:
    lines = [
        "# Envoix test matrix run",
        "",
        f"- Registry revision: {aggregate['registry_revision']}",
        f"- Run ID: `{_markdown(aggregate['run_id'])}`",
        f"- Tested commit: `{_markdown(aggregate['tested_commit'])}`",
        f"- Build variant: `{_markdown(aggregate['build_variant'])}`",
        f"- Selection: `{_markdown(aggregate['selection'])}`",
        f"- Dry-run: `{str(aggregate['dry_run']).lower()}`",
        f"- Run status: **{_markdown(aggregate['run_status'])}**",
        f"- Release gate: **{_markdown(aggregate['release_gate'])}**",
        "",
        "## Execution summary",
        "",
        "| Status | Count |",
        "|---|---:|",
    ]
    for status in sorted(EXECUTION_STATUSES):
        lines.append(f"| {status} | {aggregate['summary'][status]} |")
    lines.extend(
        [
            "",
            "## Executions",
            "",
            "| Case | Repetition | Direction | Scenario | Support | Execution | Failure | Evidence |",
            "|---|---:|---|---|---|---|---|---|",
        ]
    )
    for result in aggregate["results"]:
        lines.append(
            "| "
            + " | ".join(
                _markdown(value if value is not None else "")
                for value in (
                    result["case_id"],
                    result["repetition"],
                    f"{result['sender']} -> {result['receiver']}",
                    result["scenario"],
                    result["support_status"],
                    result["execution_status"],
                    result["failure_code"],
                    ", ".join(result["endpoint_results"]),
                )
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "Dry-run and deferred executions are never counted as passes.",
            "",
        ]
    )
    return "\n".join(lines)


def redact_text(value: str) -> str:
    for pattern, replacement in REDACTIONS:
        value = pattern.sub(replacement, value)
    return value


def sensitive_findings(value: str) -> list[str]:
    return [
        label
        for label, pattern in SENSITIVE_TEXT_PATTERNS.items()
        if pattern.search(value)
    ]


def _markdown(value: object) -> str:
    return str(value).replace("\\", "\\\\").replace("|", "\\|")


def render_registry_report(
    registry: dict[str, Any],
    cases: list[dict[str, Any]] | None = None,
) -> str:
    selected = registry["cases"] if cases is None else cases
    counts = Counter(case["support_status"] for case in selected)
    lines = [
        "# Envoix test matrix registry",
        "",
        f"- Schema version: {registry['schema_version']}",
        f"- Registry revision: {registry['registry_revision']}",
        f"- Selected cases: {len(selected)}",
        "",
        "## Support status",
        "",
        "| Status | Cases |",
        "|---|---:|",
    ]
    for status in sorted(SUPPORT_STATUSES):
        lines.append(f"| {status} | {counts[status]} |")
    lines.extend(
        [
            "",
            "## Cases",
            "",
            "| Case | Layer | Gate | Direction | Profile | Fault | Support | Execution |",
            "|---|---|---|---|---|---|---|---|",
        ]
    )
    for case in selected:
        direction = f"{case['sender']} -> {case['receiver']}"
        lines.append(
            "| "
            + " | ".join(
                _markdown(value)
                for value in (
                    case["case_id"],
                    case["test_layer"],
                    case["gate"],
                    direction,
                    case["transfer_profile"],
                    case["fault_profile"],
                    case["support_status"],
                    "not_run",
                )
            )
            + " |"
        )
    lines.extend(
        [
            "",
            "This is a registry-only report. `not_run` is never a pass.",
            "",
        ]
    )
    return "\n".join(lines)


def _add_filters(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--status", action="append", choices=sorted(SUPPORT_STATUSES))
    parser.add_argument("--layer", choices=sorted(TEST_LAYERS))
    parser.add_argument("--gate")
    parser.add_argument("--tag")


def _write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate-registry", help="Validate a registry")
    validate.add_argument("registry", type=Path)

    endpoint = subparsers.add_parser(
        "validate-endpoint-result",
        help="Validate and retain one sanitized endpoint result",
    )
    endpoint.add_argument("input", type=Path)
    endpoint.add_argument("--run-id", required=True)
    endpoint.add_argument("--case", required=True, dest="case_id")
    endpoint.add_argument("--repetition", required=True, type=int)
    endpoint.add_argument("--role", required=True, choices=sorted(ENDPOINT_ROLES))
    endpoint.add_argument(
        "--platform",
        required=True,
        choices=sorted(ENDPOINTS - {"rust_loopback", "cli"}),
    )
    endpoint.add_argument("--output", required=True, type=Path)

    list_cases = subparsers.add_parser("list-cases", help="List validated cases")
    list_cases.add_argument("registry", type=Path)
    _add_filters(list_cases)
    list_cases.add_argument("--json", action="store_true", dest="as_json")

    render = subparsers.add_parser(
        "render-report",
        help="Render a registry-only not-run report",
    )
    render.add_argument("registry", type=Path)
    _add_filters(render)
    render.add_argument("--output", type=Path)

    resolve = subparsers.add_parser(
        "resolve-cases",
        help="Resolve explicit registry cases into a deterministic run plan",
    )
    resolve.add_argument("registry", type=Path)
    resolve.add_argument("--case", action="append", default=[])
    resolve.add_argument("--gate")
    resolve.add_argument("--tag")
    resolve.add_argument("--legacy-scenario", action="append", default=[])
    resolve.add_argument("--legacy-direction", action="append", default=[])
    resolve.add_argument("--run-id", required=True)
    resolve.add_argument("--commit", required=True, dest="tested_commit")
    resolve.add_argument(
        "--build-variant",
        required=True,
        choices=sorted(BUILD_VARIANTS),
    )
    resolve.add_argument("--repetitions", type=int)
    resolve.add_argument("--dry-run", action="store_true")
    resolve.add_argument("--output", required=True, type=Path)

    executions = subparsers.add_parser(
        "list-executions",
        help="List the validated execution rows in a run plan",
    )
    executions.add_argument("registry", type=Path)
    executions.add_argument("plan", type=Path)

    record = subparsers.add_parser(
        "record-result",
        help="Write one validated runner execution record",
    )
    record.add_argument("--run-id", required=True)
    record.add_argument("--case", required=True, dest="case_id")
    record.add_argument("--repetition", required=True, type=int)
    record.add_argument("--status", required=True, choices=sorted(EXECUTION_STATUSES))
    record.add_argument("--failure-code")
    record.add_argument("--sanitized-log", action="append", default=[])
    record.add_argument("--endpoint-result", action="append", default=[])
    record.add_argument("--output", required=True, type=Path)

    aggregate = subparsers.add_parser(
        "aggregate-run",
        help="Aggregate execution records and render the run report",
    )
    aggregate.add_argument("registry", type=Path)
    aggregate.add_argument("plan", type=Path)
    aggregate.add_argument("records_root", type=Path)
    aggregate.add_argument("--json-output", required=True, type=Path)
    aggregate.add_argument("--report-output", required=True, type=Path)

    redact = subparsers.add_parser("redact", help="Redact a UTF-8 text file")
    redact.add_argument("input", type=Path)
    redact.add_argument("output", type=Path)

    check = subparsers.add_parser(
        "redaction-check",
        help="Fail if a retained UTF-8 file contains sensitive text",
    )
    check.add_argument("paths", nargs="+", type=Path)
    return parser.parse_args(argv)


def _selected_from_args(
    registry: dict[str, Any],
    args: argparse.Namespace,
) -> list[dict[str, Any]]:
    return select_cases(
        registry,
        statuses=set(args.status) if args.status else None,
        layer=args.layer,
        gate=args.gate,
        tag=args.tag,
    )


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.command == "validate-endpoint-result":
        try:
            result = read_endpoint_result(
                args.input,
                run_id=args.run_id,
                case_id=args.case_id,
                repetition=args.repetition,
                role=args.role,
                platform=args.platform,
            )
            _write_json(args.output, result)
        except (OSError, ValueError) as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0

    if args.command == "record-result":
        try:
            record = execution_record(
                run_id=args.run_id,
                case_id=args.case_id,
                repetition=args.repetition,
                status=args.status,
                failure_code=args.failure_code,
                sanitized_logs=args.sanitized_log,
                endpoint_results=args.endpoint_result,
            )
            _write_json(args.output, record)
        except (OSError, ValueError) as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0

    if args.command == "redact":
        try:
            value = args.input.read_text(encoding="utf-8", errors="replace")
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(redact_text(value), encoding="utf-8")
        except OSError as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0

    if args.command == "redaction-check":
        findings: list[str] = []
        for path in args.paths:
            try:
                value = path.read_text(encoding="utf-8", errors="replace")
            except OSError as error:
                findings.append(f"{path}: could not read: {error}")
                continue
            for label in sensitive_findings(value):
                findings.append(f"{path}: contains sensitive {label}")
        if findings:
            print(
                "\n".join(f"error: {finding}" for finding in findings), file=sys.stderr
            )
            return 1
        return 0

    try:
        registry = read_registry(args.registry)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    if args.command == "validate-registry":
        print(
            f"matrix registry valid: revision={registry['registry_revision']} "
            f"profiles={len(registry['profiles'])} cases={len(registry['cases'])}"
        )
        return 0

    if args.command == "resolve-cases":
        try:
            selected, warnings, selection = resolve_cases(
                registry,
                case_ids=args.case,
                gate=args.gate,
                tag=args.tag,
                legacy_scenarios=args.legacy_scenario,
                legacy_directions=args.legacy_direction,
            )
            plan = build_run_plan(
                registry,
                selected,
                run_id=args.run_id,
                tested_commit=args.tested_commit,
                build_variant=args.build_variant,
                selection=selection,
                dry_run=args.dry_run,
                repetitions=args.repetitions,
            )
            _write_json(args.output, plan)
        except (OSError, ValueError) as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        for warning in warnings:
            print(f"warning: {warning}", file=sys.stderr)
        print(
            f"resolved {len(selected)} cases into {len(plan['executions'])} executions"
        )
        return 0

    if args.command == "list-executions":
        try:
            plan = read_run_plan(args.plan, registry)
        except ValueError as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        cases_by_id = {case["case_id"]: case for case in registry["cases"]}
        for execution in plan["executions"]:
            print(
                "\t".join(
                    [
                        *(
                            str(execution[field])
                            for field in (
                                "case_id",
                                "repetition",
                                "sender",
                                "receiver",
                                "scenario",
                                "timeout_seconds",
                            )
                        ),
                        cases_by_id[execution["case_id"]]["test_layer"],
                        execution["disposition"],
                    ]
                )
            )
        return 0

    if args.command == "aggregate-run":
        try:
            plan = read_run_plan(args.plan, registry)
            aggregate = aggregate_run(registry, plan, args.records_root)
            _write_json(args.json_output, aggregate)
            args.report_output.parent.mkdir(parents=True, exist_ok=True)
            args.report_output.write_text(
                render_run_report(aggregate),
                encoding="utf-8",
            )
        except (OSError, ValueError) as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 1 if aggregate["run_status"] == "failure" else 0

    selected = _selected_from_args(registry, args)
    if args.command == "list-cases":
        if args.as_json:
            print(json.dumps(selected, indent=2, sort_keys=True))
        else:
            for case in selected:
                print(
                    f"{case['case_id']}\t{case['support_status']}\t"
                    f"{case['sender']}->{case['receiver']}\t{case['title']}"
                )
        return 0

    report = render_registry_report(registry, selected)
    if args.output:
        try:
            args.output.write_text(report, encoding="utf-8")
        except OSError as error:
            print(f"error: could not write {args.output}: {error}", file=sys.stderr)
            return 1
    else:
        print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
