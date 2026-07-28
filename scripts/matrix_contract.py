#!/usr/bin/env python3
"""Validate and inspect the versioned end-to-end test matrix contract."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path
from typing import Any, Iterable


SCHEMA_VERSION = 1
MAX_CASES = 512
MAX_PROFILES = 64
MAX_TEXT_LENGTH = 400
MAX_TIMEOUT_SECONDS = 86_400
MAX_TRANSFER_BYTES = 1 << 40

IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9_.-]{0,95}$")
PROFILE_IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9_-]{0,63}$")
REVISION = re.compile(r"^\d{4}-\d{2}-\d{2}$")
SENSITIVE_TEXT_PATTERNS = {
    "Room Code-shaped secret": re.compile(
        r"(?i)(?<![a-z0-9])\d{6}-[a-z0-9]{4,8}-[a-z0-9]{4,8}(?![a-z0-9])"
    ),
    "invitation URI": re.compile(r"(?i)envoix://invite/v2/"),
    "absolute private path": re.compile(
        r"(?:/Users/|/home/|/data/user/|/private/var/)"
    ),
}

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


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser("validate-registry", help="Validate a registry")
    validate.add_argument("registry", type=Path)

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
