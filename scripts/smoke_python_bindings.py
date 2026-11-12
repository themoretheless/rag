#!/usr/bin/env python3
"""Isolated MCP Python name bindings; analyzed source is never executed."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-python-bindings-') as tmp:
        root = Path(tmp).resolve()
        source = root / 'sources'
        fixtures = {
            'alpha/lib.py': 'def 𝒇(value):\n    return value\ndef other(value):\n    return 7\n',
            'alpha/app.py': 'from 𝒍ib import f as 𝒈\ndef entry(x):\n    return g(x)\ndef shadowed(𝒈,x):\n    return g(x)\ndef late(x):\n    g(x)\n    𝒈 = unknown\ndef opaque(xs):\n    return [g(x) for 𝒈 in xs]\n',
            'beta/lib.py': 'def f(value):\n    return value\n',
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
            def candidates(name):
                result = mcp.call('callees', dict(symbol=items[name]['id']))
                return {c for edge in result['items'] for c in edge['candidates']}
            assert candidates('entry') == {items['𝒇']['id']}
            for name in ('shadowed', 'late', 'opaque'):
                assert not candidates(name), name
            result = mcp.call('data_slice', dict(symbol=items['entry']['id'], project='alpha'))
            assert any(n['node']['function'] == items['𝒇']['id'] for n in result['items']), result
            mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
            assert symbols()['entry']['id'] == items['entry']['id']
            path = source/'alpha/app.py'
            path.write_text(fixtures['alpha/app.py'].replace('import f as', 'import other as'))
            mcp.call('ingest_file', dict(path=str(path), wing='alpha'))
            assert not mcp.call('symbol', dict(path=str(path)))['items']
            mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
            items = symbols()
            assert candidates('entry') == {items['other']['id']}
            report = dict(passed=True, checks=9, production_database_touched=False, source_executed=False, versions=mcp.call('index_status', {})['versions'])
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report))


if __name__ == '__main__':
    main()
