"""Workload isolation and environment validity guards, without a production store."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from bench_real_index import apply, environment


class BenchmarkGuards(unittest.TestCase):
    def test_recipe_cannot_mutate_outside_owned_source_or_follow_links(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owned = root / 'source'
            owned.mkdir()
            outside = root / 'outside.rs'
            outside.write_text('fn original(){}')
            (owned / 'link.rs').symlink_to(outside)
            (owned / 'link_dir').symlink_to(root, target_is_directory=True)
            for name in ['../outside.rs', str(outside), 'link.rs', 'link_dir/outside.rs']:
                with self.assertRaises(ValueError):
                    apply(owned, dict(edits=[dict(path=name, before='original', after='changed')]))
            self.assertEqual(outside.read_text(), 'fn original(){}')

    def test_nonunique_or_noop_replacements_fail_without_silently_changing_workload(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            file = root / 'lib.rs'
            file.write_text('fn first(){} fn second(){}')
            for edit in [dict(before='fn', after='pub fn'), dict(before='absent', after='present'),
                         dict(before='first', after='first'), dict(before='first', after='x', append='extra')]:
                with self.assertRaises(ValueError):
                    apply(root, dict(edits=[dict(path='lib.rs', **edit)]))
                self.assertEqual(file.read_text(), 'fn first(){} fn second(){}')
            receipts = apply(root, dict(edits=[dict(path='lib.rs', before='first', after='renamed')]))
            self.assertNotEqual(receipts[0]['before_sha256'], receipts[0]['after_sha256'])

    def test_compilers_block_even_when_idle_but_measured_worker_is_not_competition(self):
        output = f'{os.getpid()} 100 python\n123 0 /toolchain/cargo\n124 0 /toolchain/rustc\n125 80 /tmp/rag-mcp\n'
        with patch('bench_real_index.subprocess.run', return_value=subprocess.CompletedProcess([], 0, stdout=output)):
            result = environment(ignore_pids=[125])
        self.assertFalse(result['quiet'])
        self.assertEqual({p['pid'] for p in result['blocking_processes']}, {123, 124})
        self.assertTrue(all(p['compiler'] for p in result['blocking_processes']))
        with patch('bench_real_index.subprocess.run', return_value=subprocess.CompletedProcess([], 0, stdout='125 80 /tmp/rag-mcp\n')):
            self.assertTrue(environment(ignore_pids=[125])['quiet'])
            self.assertFalse(environment()['quiet'])


if __name__ == '__main__':
    unittest.main()
