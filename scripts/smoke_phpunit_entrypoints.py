#!/usr/bin/env python3
"""Isolated real MCP PHPUnit declaration catalog; never executes PHP or PHPUnit."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-phpunit-catalog-') as temp:
        root = Path(temp).resolve()
        source = root / 'sources'
        source.mkdir()
        path = source / 'Tests.php'
        body = r'''<?php namespace Suite;
use PHPUnit\Framework\TestCase as Base;
use PHPUnit\Framework\Attributes\Test as Check;
abstract class AbstractTest extends Base {public function testAbstract(){}}
class Good extends AbstractTest {
 public function testWorks(){} #[Check] public function works(){}
 private function testHidden(){} public static function testStatic(){}
 #[Fake] public function unrelated(){}
}
class Fake {public function testWrong(){}}
function testLoose(){}
'''
        path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            def sync():
                result = mcp.call('sync_sources', dict(path=str(source), wing='alpha'))
                assert not result.get('errors'), result
            def catalog():
                result = mcp.call('entrypoints', dict(project='alpha', limit=500))
                return [e for e in result['items'] if e['kind'] == 'phpunit_test_candidate']
            sync()
            symbols = {s['id']: s for s in mcp.call('symbol', dict(project='alpha', limit=500))['items']}
            entries = catalog()
            assert {symbols[e['symbol_id']]['qualified_name'] for e in entries} == {'Suite\\Good::works', 'Suite\\Good::testWorks', 'Suite\\AbstractTest::testAbstract'}, entries
            assert {e['reference_kind'] for e in entries} == {'phpunit_test_name_candidate', 'phpunit_test_attribute_candidate'}, entries
            for entry in entries:
                symbol = symbols[entry['symbol_id']]
                assert entry['document_id'] == symbol['document_id'] and entry['content_hash'] == symbol['content_hash'], entry
            sync()
            assert entries == catalog()
            path.write_text(body.replace('extends AbstractTest', ''))
            mcp.call('ingest_file', dict(path=str(path), wing='alpha'))
            assert not catalog(), 'stale catalog must be excluded'
            sync()
            assert not catalog(), 'removed PHPUnit base must remove candidates'
            report = dict(status='passed', versions=mcp.call('index_status', {})['versions'], checks=['base_alias', 'indirect_abstract_base', 'attribute_alias', 'public_instance_only', 'framework_base_required', 'source_identity', 'warm_equivalence', 'stale_exclusion', 'sync_removes_candidates'], PHP_executed=False, PHPUnit_executed=False, runtime_test_discovery_complete=False, affected_tests_trait_contexts_exercised=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report))


if __name__ == '__main__':
    main()
