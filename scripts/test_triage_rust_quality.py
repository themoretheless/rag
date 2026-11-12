import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from qualify_rust_corpus import metrics
from triage_rust_quality import audit_run, excerpt, owned_path


class QualityTriageTests(unittest.TestCase):
    def fixture(self, root):
        (root / "src").mkdir()
        (root / "src/lib.rs").write_text("fn run() { missing(); }\n")
        label = dict(path="src/lib.rs", range=[0, 11, 0, 18], symbol="local 1",
                     syntax_class="direct_call", expected=[dict(path="src/lib.rs", range=[0, 3, 0, 6])])
        case = dict(**label, observations=[], observed_definitions=[], true_positive=0,
                    false_positive=0, false_negative=1, exact=False)
        report = dict(status="measured", labelled_native_mcp_parity=True, parity_sites=1,
                      input_sha256={"src/lib.rs": hashlib.sha256((root / "src/lib.rs").read_bytes()).hexdigest()},
                      label_cases_sha256=hashlib.sha256(json.dumps([label], sort_keys=True).encode()).hexdigest(),
                      metrics=metrics([case]))
        (root / "report.json").write_text(json.dumps(report))
        (root / "oracle-labels.json").write_text(json.dumps(dict(cases=[label])))
        scored = dict(cases=[case], metrics=metrics([case]))
        (root / "scored-cases.json").write_text(json.dumps(scored))
        return scored

    def test_missing_observation_keeps_positive_denominator(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            result = audit_run(root, root)
            self.assertEqual(result["metrics"]["local_positive_sites"], 1)
            self.assertEqual(result["groups"], {"no_observation": 1})
            self.assertEqual(result["misses"][0]["source"]["token"], "missing")

    def test_forged_score_and_target_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scored = self.fixture(root)
            for key, value in [("false_negative", 0), ("expected", [])]:
                changed = copy.deepcopy(scored)
                changed["cases"][0][key] = value
                (root / "scored-cases.json").write_text(json.dumps(changed))
                with self.assertRaises(ValueError):
                    audit_run(root, root)

    def test_source_drift_rejected_before_triage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.fixture(root)
            (root / "src/lib.rs").write_text("fn another() {}\n")
            with self.assertRaisesRegex(ValueError, "changed corpus input"):
                audit_run(root, root)

    def test_byte_columns_and_path_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "input.rs").write_text("éfoo\n")
            self.assertEqual(excerpt(root, "input.rs", [0, 2, 0, 5])["token"], "foo")
            with self.assertRaises(ValueError):
                owned_path(root, "../outside.rs")
            (root / "escape").symlink_to(root.parent, target_is_directory=True)
            with self.assertRaises(ValueError):
                owned_path(root, "escape/outside.rs")


if __name__ == "__main__":
    unittest.main()
