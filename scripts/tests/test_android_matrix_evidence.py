import base64
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import android_matrix_evidence as evidence
import matrix_contract


class AndroidEvidenceTests(unittest.TestCase):
    def extract(self, text):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "instrumentation.log"
            path.write_text(text)
            return evidence.extract_evidence(path)

    def line(self, value):
        return evidence.PREFIX + base64.b64encode(json.dumps(value).encode()).decode() + "\n"

    def test_reads_evidence_among_instrumentation_status(self):
        value = {"platform": "android", "role": "sender", "terminal_state": "completed"}
        self.assertEqual(self.extract("INSTRUMENTATION_STATUS_CODE: 1\n" + self.line(value) + "OK (1 test)\n"), value)

    def test_rejects_missing_duplicate_malformed_and_non_object_evidence(self):
        for value in ["OK (1 test)\n", self.line({}) * 2, evidence.PREFIX + "!invalid!", self.line([])]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.extract(value)

    def test_rejects_oversized_evidence(self):
        with self.assertRaises(ValueError):
            self.extract(evidence.PREFIX + "a" * (evidence.MAX_ENCODED_BYTES + 1))

    def test_encoded_private_data_never_survives_log_redaction(self):
        encoded = self.line({"path": "/data/user/0/private", "invitation": "envoix://invite/v2/secret"})
        redacted = matrix_contract.redact_text(encoded)
        self.assertNotIn(encoded.strip(), redacted)
        self.assertNotIn("envoixMatrixEvidence=", redacted)
        self.assertIn("REDACTED_ANDROID_ENDPOINT_EVIDENCE", redacted)


if __name__ == "__main__":
    unittest.main()
