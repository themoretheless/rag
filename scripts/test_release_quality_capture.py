"""Keep host/proc-macro artifacts out of the runtime-library selector."""
from pathlib import Path
import tempfile
import unittest
from build_release_quality_capture import libraries


def artifact(name, path, optimization='3'):
    return dict(reason='compiler-artifact', target=dict(name=name, kind=['lib']),
                profile=dict(opt_level=optimization, debug_assertions=False, test=False),
                filenames=[str(path)])


class ReleaseLibrarySelection(unittest.TestCase):
    def test_optimized_runtime_is_selected_instead_of_host_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            host = root / 'target/host/libserde_json-host.rlib'
            runtime = root / 'target/runtime/libserde_json-runtime.rlib'
            event, path = libraries(root, [artifact('serde_json', host, '0'), artifact('serde_json', runtime)], 'serde_json')
            self.assertEqual(path, runtime)
            self.assertEqual(event['profile']['opt_level'], '3')

    def test_ambiguous_or_external_runtime_paths_cannot_be_selected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            a = artifact('rag_mcp', root / 'target/a/lib.rlib')
            b = artifact('rag_mcp', root / 'target/b/lib.rlib')
            for events in [[], [a, b], [artifact('rag_mcp', root / 'outside.rlib')]]:
                with self.assertRaises(ValueError):
                    libraries(root, events, 'rag_mcp')


if __name__ == '__main__':
    unittest.main()
