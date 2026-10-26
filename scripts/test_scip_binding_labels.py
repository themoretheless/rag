import json
import tempfile
from pathlib import Path
import unittest
from scip_binding_labels import fields, labels, source_range, external_method_descriptor
from qualify_rust_corpus import metrics


def integer(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def scalar(field, value):
    return integer(field << 3) + integer(value)


def blob(field, value):
    value = value.encode() if isinstance(value, str) else value
    return integer((field << 3) | 2) + integer(len(value)) + value


class ScipLabelsTests(unittest.TestCase):
    def test_external_method_descriptor_consumes_complete_scip_grammar(self):
        local = ("fixture", "1.0.0")
        for path in ["lib/cmp/max().", "lib/impl#[`*const T`]mark().", "lib/impl#[`With``Backtick`]run(disambiguator)."]:
            self.assertTrue(external_method_descriptor("rust-analyzer cargo core 0.0.0 " + path, local), path)
        for symbol in [
            "rust-analyzer cargo fixture 1.0.0 lib/local().",
            "local 1", "rust-analyzer cargo core 0.0.0 lib/`fake().`.",
            "rust-analyzer cargo core 0.0.0 lib/value.",
            "rust-analyzer cargo core 0.0.0 lib/bad().garbage",
            "rust-analyzer cargo core 0.0.0 lib/`unterminated().",
            "rust-analyzer cargo core 0.0.0 lib/run(escaped space).",
            "rust-analyzer cargo core 0.0.0 lib/run().(parameter)",
        ]:
            self.assertFalse(external_method_descriptor(symbol, local), symbol)

    def test_external_descriptor_labels_are_opt_in_and_require_independent_call_anchor(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="1.0.0"\n')
            source = "fn run(){ x.external(); imported; missing(); }\n"
            (root / "src/lib.rs").write_text(source)
            symbols = ["rust-analyzer cargo core 0.0.0 lib/external().",
                       "rust-analyzer cargo core 0.0.0 lib/imported().",
                       "rust-analyzer cargo fixture 1.0.0 lib/missing()."]
            document = blob(1, "src/lib.rs") + scalar(6, 1)
            ast = {"kind": "SOURCE_FILE", "children": []}
            for name, symbol in zip(["external", "imported", "missing"], symbols):
                start = source.index(name)
                end = start + len(name)
                document += blob(2, blob(1, bytes([0, start, end])) + blob(2, symbol))
                token = dict(type="Token", kind="IDENT", start=[start, 0, start], end=[end, 0, end])
                name_node = dict(kind="NAME_REF", start=[start, 0, start], end=[end, 0, end], children=[token])
                ast["children"].append(dict(kind="USE_TREE" if name == "imported" else "METHOD_CALL_EXPR", children=[name_node]))
            document += blob(3, blob(1, "rust-analyzer cargo fixture 1.0.0 lib/") + scalar(5, 29))
            index = root / "index.scip"
            index.write_bytes(blob(2, document))
            syntax = root / "ast/src"
            syntax.mkdir(parents=True)
            (syntax / "lib.rs.json").write_text(json.dumps(ast))
            self.assertEqual(labels(index, root, root / "ast")["cases"], [])
            result = labels(index, root, root / "ast", include_external_descriptors=True)
            self.assertEqual(result["contract_version"], 2)
            self.assertEqual(len(result["cases"]), 1)
            self.assertEqual(result["cases"][0]["symbol"], symbols[0])
            self.assertEqual(result["cases"][0]["kinds"], [])
            self.assertEqual(result["cases"][0]["expected"], [])
            self.assertIn("SCIP Method descriptor", result["cases"][0]["callable_evidence"])
            self.assertEqual(labels(index, root, include_external_descriptors=True)["cases"], [])
            start = source.index("external")
            conflict = blob(2, blob(1, bytes([0, start, start + len("external")])) + blob(2, symbols[2]))
            index.write_bytes(blob(2, document + conflict))
            ambiguous = labels(index, root, root / "ast", include_external_descriptors=True)
            self.assertEqual(ambiguous["cases"], [])
            self.assertEqual(ambiguous["excluded"]["external_descriptor_ambiguous_anchor"], 1)


    def test_quality_metrics_keep_abstention_and_false_external_binding_visible(self):
        cases = [
            dict(expected=[1], true_positive=1, false_positive=0, false_negative=0, exact=True),
            dict(expected=[2], true_positive=0, false_positive=0, false_negative=1, exact=False),
            dict(expected=[], true_positive=0, false_positive=1, false_negative=0, exact=False),
        ]
        result = metrics(cases)
        self.assertEqual(result["precision"], .5)
        self.assertEqual(result["recall"], .5)
        self.assertEqual(result["external_negative_sites"], 1)
        self.assertEqual(result["exact_site_rate"], 1 / 3)
        self.assertIsNone(metrics([])["precision"])
        self.assertIsNone(metrics([])["recall"])

    def test_range_forms_are_equivalent_and_conflicts_are_rejected(self):
        old = blob(1, b"\x03\x04\x09")
        typed = blob(8, scalar(1, 3) + scalar(2, 4) + scalar(3, 9))
        self.assertEqual(source_range(fields(old + typed)), [3, 4, 3, 9])
        with self.assertRaises(ValueError):
            source_range(fields(old + blob(8, scalar(1, 2) + scalar(2, 4) + scalar(3, 9))))
        for data in [b"\x80", b"\x12\x05x", b"\x00"]:
            with self.assertRaises(ValueError):
                fields(data)

    def test_document_local_ids_do_not_collide_and_missing_local_definitions_are_not_external(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="1.0.0"\n')
            (root / "src/a.rs").write_text("fn run(){}\nfn caller(){run();}\n")
            (root / "src/b.rs").write_text("fn other(){let local=1;}\n")
            definition = blob(1, b"\x00\x03\x06") + blob(2, "local 1") + scalar(3, 1)
            reference = blob(1, b"\x01\x0c\x0f") + blob(2, "local 1")
            missing = blob(1, b"\x01\x0c\x0f") + blob(2, "rust-analyzer cargo fixture 1.0.0 lib/missing().")
            method = blob(1, "local 1") + scalar(5, 17)
            missing_info = blob(1, "rust-analyzer cargo fixture 1.0.0 lib/missing().") + scalar(5, 26)
            module = blob(1, "rust-analyzer cargo fixture 1.0.0 lib/a/") + scalar(5, 29)
            first = blob(1, "src/a.rs") + scalar(6, 1) + blob(2, definition) + blob(2, reference) + blob(2, missing) + blob(3, method) + blob(3, missing_info) + blob(3, module)
            other = blob(1, "src/b.rs") + scalar(6, 1) + blob(3, blob(1, "local 1") + scalar(5, 61))
            path = root / "index.scip"
            path.write_bytes(blob(2, first) + blob(2, other))
            result = labels(path, root)
            self.assertEqual(len(result["cases"]), 1)
            self.assertEqual(result["cases"][0]["expected"], [dict(path="src/a.rs", range=[0, 3, 0, 6])])
            self.assertEqual(result["conflicting_kind_symbols"], 0)
            self.assertEqual(result["excluded"]["missing_local_oracle_definition"], 1)


if __name__ == "__main__":
    unittest.main()
