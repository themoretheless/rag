"""Release artifact evidence and immutable input checks without compiling a fixture."""
import json
from pathlib import Path
import tempfile
import unittest
from build_index_release import compiler_artifact, digest, snapshot, verify_snapshot


class ReleaseEvidenceTests(unittest.TestCase):
    def test_release_artifact_requires_success_unique_server_and_optimized_profile(self):
        artifact = dict(reason='compiler-artifact', executable='/owned/target/release/rag-mcp',
                        target=dict(name='rag-mcp', kind=['bin']),
                        profile=dict(opt_level='3', debug_assertions=False, test=False))
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'cargo.jsonl'
            def events(values):
                path.write_text(''.join(json.dumps(v)+'\n' for v in values))
            events([artifact, dict(reason='build-finished', success=True)])
            self.assertEqual(compiler_artifact(path), artifact)
            for bad in [
                [artifact, dict(reason='build-finished', success=False)],
                [artifact, artifact, dict(reason='build-finished', success=True)],
                [dict(artifact, profile=dict(opt_level='0', debug_assertions=False, test=False)), dict(reason='build-finished', success=True)],
                [dict(artifact, profile=dict(opt_level='3', debug_assertions=True, test=False)), dict(reason='build-finished', success=True)],
                [dict(artifact, profile=dict(opt_level='3', debug_assertions=False, test=True)), dict(reason='build-finished', success=True)],
            ]:
                events(bad)
                with self.assertRaises(ValueError):
                    compiler_artifact(path)

    def test_snapshot_rejects_changed_sources_and_postbuild_tree_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / 'repo'
            repo.mkdir()
            source = repo / 'lib.rs'
            source.write_text('fn original(){}')
            source.chmod(0o644)
            files = {'lib.rs': dict(sha256=digest(source), executable=False)}
            owned = root / 'frozen'
            snapshot(repo, owned, files)
            verify_snapshot(owned, files)
            source.write_text('fn changed(){}')
            with self.assertRaises(ValueError):
                snapshot(repo, root / 'rejected', files)
            (owned / 'extra.rs').write_text('fn extra(){}')
            with self.assertRaises(ValueError):
                verify_snapshot(owned, files)
            (owned / 'extra.rs').unlink()
            (owned / 'lib.rs').write_text('fn edited(){}')
            with self.assertRaises(ValueError):
                verify_snapshot(owned, files)


if __name__ == '__main__':
    unittest.main()
