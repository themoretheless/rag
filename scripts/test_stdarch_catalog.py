from pathlib import Path
import tempfile
import unittest

from generate_stdarch_catalog import generate


class StdarchCatalogTests(unittest.TestCase):
    def fixture(self, root):
        for name in ["arm_shared/neon/mod.rs", "aarch64/neon/mod.rs", "x86/mod.rs",
                     "wasm32/simd128.rs", "mod.rs", "macros.rs", "aarch64/mod.rs", "arm/mod.rs", "arm_shared/mod.rs", "wasm32/mod.rs"]:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("pub struct Real;\n")

    def test_public_names_are_sorted_deduplicated_and_comments_are_excluded(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            (root / "x86/mod.rs").write_text("/// pub struct Bogus;\npub(crate) struct Private;\npub struct Zed;\npub struct Alpha;\npub struct Zed;\n")
            source, receipt = generate(root)
            self.assertEqual(receipt["catalog"]["x86"], ["Alpha", "Zed"])
            self.assertEqual(receipt["catalog"]["x86_64"], ["Alpha", "Zed"])
            self.assertNotIn("Bogus", source)
            self.assertNotIn("Private", source)
            self.assertEqual(generate(root), (source, receipt))

    def test_missing_expected_declarations_are_not_silently_admitted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            (root / "wasm32/simd128.rs").write_text("// no declarations\n")
            with self.assertRaisesRegex(ValueError, "missing expected"):
                generate(root)


if __name__ == "__main__":
    unittest.main()
