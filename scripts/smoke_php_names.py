#!/usr/bin/env python3
"""PHP name candidates over isolated real MCP; never execute PHP fixtures."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-names-') as temporary:
        root = Path(temporary).resolve()
        source = root / 'sources'
        fixtures = {
            'alpha/lib.php': '<?php namespace Lib; function first() {} function second() {} class Svc { public static function save(){first();} public function run(){$this->local();self::save();} private function local(){second();} }',
            'alpha/app.php': '<?php namespace App; use function Lib\\first as greet; use Lib\\Svc as Store; /** @param array{x:int} $data */ function entry($data){greet();Store::save();$handler=greet(...);$obj=new Store;} function main(){entry([]);}',
            'alpha/guards.php': '<?php namespace App; class Late {function unknown($fn){$fn();$other->save();static::save();}}',
            'beta/lib.php': '<?php namespace Lib; function first() {} class Svc {public static function save(){}}',
        }
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ('alpha', 'beta'):
                result = mcp.call('sync_sources', dict(path=str(source/project), wing=project))
                assert not result.get('errors'), result
            def symbols():
                return {s['qualified_name']: s for s in mcp.call('symbol', dict(project='alpha', limit=500))['items']}
            items = symbols()
            entry = items['App\\entry']
            calls = mcp.call('callees', dict(symbol=entry['id']))
            assert len(calls['items']) == 2, calls
            assert {c for edge in calls['items'] for c in edge['candidates']} == {items['Lib\\first']['id'], items['Lib\\Svc::save']['id']}, calls
            refs = mcp.call('references', dict(symbol=items['Lib\\first']['id'], project='alpha'))
            assert any(not r['is_call'] for r in refs['items']), refs
            impact = mcp.call('impact', dict(symbol=items['Lib\\first']['id'], impact_project='alpha'))
            assert items['App\\main']['id'] in {r['symbol_id'] for r in impact['items']}, impact
            guards = mcp.call('callees', dict(symbol=items['App\\Late::unknown']['id']))
            assert len(guards['items']) == 3 and all(not e['candidates'] for e in guards['items']), guards
            code = mcp.call('code', dict(symbol=entry['id']))
            assert code['code'] == fixtures['alpha/app.php'].encode()[entry['start_byte']:entry['end_byte']].decode(), code
            assert entry['parameter_contracts']['parameters'][0]['shape']['fields'][0]['key'] == 'x', entry
            warm = mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
            assert symbols()['App\\entry']['id'] == entry['id'], warm
            path = source/'alpha/app.php'
            path.write_text(fixtures['alpha/app.php'].replace('Lib\\first as greet', 'Lib\\second as greet'))
            mcp.call('ingest_file', dict(path=str(path), wing='alpha'))
            assert not mcp.call('symbol', dict(path=str(path)))['items']
            mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
            items = symbols()
            updated = mcp.call('callees', dict(symbol=items['App\\entry']['id']))
            assert {c for e in updated['items'] for c in e['candidates']} == {items['Lib\\second']['id'], items['Lib\\Svc::save']['id']}, updated
            report = dict(passed=True, versions=mcp.call('index_status', {})['versions'], scenarios=['namespace-aliases', 'declared-static-method', 'first-class-value-not-call', 'transitive-impact', 'dynamic-and-late-static-guards', 'source-bytes-and-phpdoc', 'warm-cache', 'stale-exclusion', 'import-only-refresh', 'project-isolation'], compiler_runtime_verified=False, production_database_touched=False, source_applications_executed=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report))


if __name__ == '__main__':
    main()
