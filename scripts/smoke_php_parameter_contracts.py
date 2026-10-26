#!/usr/bin/env python3
"""Qualify stated PHPDoc contracts over real MCP, isolated DB/mock embeddings."""
import ast
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-contracts-') as temporary:
        root = Path(temporary).resolve()
        source = root / 'sources'
        source.mkdir()
        path = source / 'contract.php'
        body = """<?php
/**
 * @param array{foo:int, optional?:array{'a,b':list<string>, '*':string}, ...<string,mixed>} $data don't parse { description
 */
function target($data, $other) {}
function bare($data) {}
/** @param array{action:string, user_id?:int} $data */
function simple($data) {}
/**
 * @param array{x:int} $missing
 * @phpstan-param array{x:int} $data
 * @psalm-param array{x?:int} $data
 */
function ambiguous($data) {}
/** @param array{x:int, 'x':string} $data */
function invalid($data) {}
"""
        body += '/**\n * ' + 'intro ' * 900 + '\n * @param list{string, int} $data\n */\nfunction longdoc($data) {}\n'
        path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            sync = mcp.call('sync_sources', dict(path=str(source), wing='php-contracts'))
            assert not sync.get('errors'), sync
            def symbols():
                return {s['name']: s for s in mcp.call('symbol', dict(path=str(path), limit=100))['items']}
            items = symbols()
            reference_path = Path('/tmp/ragraf-compare-20261023/src/ragraf/facade.py')
            comparison = {'available': False}
            if reference_path.exists():
                tree = ast.parse(reference_path.read_text())
                function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == '_parse_array_shapes')
                namespace = {}
                exec(compile(ast.Module(body=[function], type_ignores=[]), str(reference_path), 'exec'), namespace)
                expected = namespace['_parse_array_shapes']('/** @param array{action:string, user_id?:int} $data */')
                actual = {'data': {f['key']: dict(type=f['type_expression'], required=f['required']) for f in items['simple']['parameter_contracts']['parameters'][0]['shape']['fields']}}
                assert actual == expected, (actual, expected)
                comparison = dict(available=True, pure_reference_function_only=True, simple_fixture_equal=True, reference_commit='075caf86dc2f6274b9c48487e521915bcdfeaf2c')
            c = items['target']['parameter_contracts']
            assert c['complete'] and not c['runtime_types_verified'] and not c['dialect_precedence_applied'], c
            assert c['undocumented_parameters'] == ['other'], c
            p = c['parameters'][0]
            shape = p['shape']
            assert p['declaration_match'] == 'matched' and not shape['sealed'], p
            assert shape['fields'][0]['required'] and not shape['fields'][1]['required'], shape
            nested = shape['fields'][1]['nested_shape']
            assert [f['key'] for f in nested['fields']] == ['a,b', '*'], nested
            span = nested['fields'][1]['source']
            assert "'*':string" in body.encode()[span['start_byte']:span['end_byte']].decode(), span
            assert mcp.call('code', dict(symbol=items['target']['id']))['symbol']['parameter_contracts'] == c
            assert items['bare']['parameter_contracts']['status'] == 'no_docblock'
            ambiguous = items['ambiguous']['parameter_contracts']
            assert not ambiguous['complete'] and ambiguous['multiple_tag_names'] == ['data'], ambiguous
            assert ambiguous['parameters'][0]['declaration_match'] == 'not_in_captured_parameters', ambiguous
            invalid = items['invalid']['parameter_contracts']
            assert not invalid['complete'] and invalid['parameters'][0]['shape'] is None, invalid
            assert items['longdoc']['documentation']['text_truncated'], items['longdoc']['documentation']
            assert items['longdoc']['parameter_contracts']['parameters'][0]['shape']['fields'][1]['key'] == '1'
            warm = mcp.call('sync_sources', dict(path=str(source), wing='php-contracts'))
            assert symbols()['target']['parameter_contracts'] == c
            path.write_text(body.replace('foo:int', 'foo?:int'))
            mcp.call('ingest_file', dict(path=str(path), wing='php-contracts'))
            assert mcp.call('symbol', dict(path=str(path)))['items'] == []
            mcp.call('sync_sources', dict(path=str(source), wing='php-contracts'))
            updated = symbols()['target']['parameter_contracts']
            assert not updated['parameters'][0]['shape']['fields'][0]['required'], updated
            assert updated['documentation_hash'] != c['documentation_hash'], updated
            status = mcp.call('index_status', {})
            result = dict(passed=True, reference_comparison=comparison, scenarios=['nested-and-quoted-shapes', 'byte-source-proof', 'declaration-binding', 'duplicate-dialects', 'malformed-no-partial', 'full-doc-after-excerpt', 'warm-cache', 'stale-exclusion-and-sync-refresh'], versions=status['versions'], runtime_types_verified=False, warm_index=warm['code_index'])
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
            print(json.dumps(result, ensure_ascii=False))


if __name__ == '__main__':
    main()
