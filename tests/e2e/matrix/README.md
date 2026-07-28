# End-to-end matrix contract

`cases.v1.json` is the source of truth for the product test matrix described by
issue #61. Every case is explicit. The runner must not generate an unreviewed
Cartesian product.

Validate the registry:

```bash
python3 scripts/matrix_contract.py validate-registry \
  tests/e2e/matrix/cases.v1.json
```

List its cases:

```bash
python3 scripts/matrix_contract.py list-cases \
  tests/e2e/matrix/cases.v1.json
```

Render the registry-only report:

```bash
python3 scripts/matrix_contract.py render-report \
  tests/e2e/matrix/cases.v1.json
```

Support status and execution status are separate. A planned, experimental,
hardware-blocked, or unsupported row never becomes a pass because it was
skipped or omitted.

The initial registry records the current direct JNI/FFI physical coverage as
experimental L1 evidence. Product-path L2 rows remain planned until they emit
the canonical Activity, native publication, typed path, attempt, cleanup, and
sanitized evidence required by the issue.

Registry and report data must never contain Room Codes, invitation payloads,
tokens, credentials, stable device identifiers, or absolute private paths.
