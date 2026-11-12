#!/usr/bin/env python3
"""Compare immutable Python value captures via isolated MCP, never execute sources."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-value-diff-') as tmp:
        root = Path(tmp).resolve()
        source = root/'sources'
        app = 'from lib import first as chosen\ndef entry(flag, left, right):\n    result = left\n    if flag:\n        result = chosen(right)\n    return result\n'
        lib = 'def first(value):\n    return value\ndef second(value):\n    return value\n'
        fixtures = {'alpha/app.py': app, 'alpha/lib.py': lib,
                    'beta/app.py': 'def entry(foreign):\n    return foreign + 99\n'}
        for name, body in fixtures.items():
            path = source/name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ('alpha', 'beta'):
                result = mcp.call('sync_sources', dict(path=str(source/project), wing=project))
                assert not result.get('errors'), result
            def revision():
                return mcp.call('index_status', {})['index_rev']
            def sync(name, body):
                path = source/name
                path.write_text(body)
                mcp.call('ingest_file', dict(path=str(path), wing='alpha'))
                mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
                return revision()
            def diff(before, after, limit=100):
                outer = mcp.call('diff', dict(before=before, after=after, project='alpha', limit=limit))
                value = outer['data_flow_comparison']
                assert value['available'] and value['normalization_format'] == 3 and value['capture_format'] == 7, value
                assert not value['source_semantic_change_proven'] and not value['runtime_path_proven'] and not value['absence_proven'] and not value['bodies_included'], value
                assert value['normalization_work'] <= value['limits']['normalization_work'], value
                for change in value['nodes_changed']:
                    for side in ('before', 'after'):
                        if change[side] is not None:
                            assert change[side]['evidence']['source']['project'] == 'alpha', change
                assert 'def entry' not in json.dumps(value), value
                return outer, value
            def totals(value):
                return tuple(value[k] for k in ('nodes_changed_total', 'edges_added_total', 'edges_removed_total'))
            capability=mcp.call('index_status', {})['capabilities']['frozen_value_diff']
            assert capability['available'] and capability['normalization_format']==3 and not capability['source_semantic_change_proven'],capability
            a = revision()
            formatted = '# shifted source offsets\n'+app.replace('left', '𝒍eft').replace('result =', 'result  =')
            b = sync('alpha/app.py', formatted)
            _, formatting = diff(a, b)
            assert totals(formatting) == (0, 0, 0) and formatting['complete_for_captured_scope'], formatting
            c = sync('alpha/app.py', formatted.replace('import first as', 'import second as'))
            _, redirected = diff(b, c)
            assert totals(redirected) == (0, 2, 2), redirected
            assert {e['edge']['relation'] for e in redirected['edges_added']} == {'call_argument', 'return_to_call'}, redirected
            d = sync('alpha/lib.py', lib.replace('def second(value):\n    return value', 'def second(value):\n    return value + 1'))
            _, edited = diff(c, d)
            assert edited['nodes_changed_total'] > 0 and edited['edges_added_total'] > 0, edited
            e = sync('alpha/lib.py', lib.replace('def second(value):\n    return value', 'def second(value):\n    try:\n        return value\n    except build_type():\n        return 0'))
            _, coverage = diff(d, e)
            assert totals(coverage) == (0, 0, 0) and coverage['excluded_functions_total'] == 1 and not coverage['complete_for_captured_scope'], coverage
            assert coverage['unverified_call_boundaries']['after'] > 0, coverage
            _, frozen = diff(a, b)
            assert frozen == formatting, (frozen, formatting)
            f = sync('alpha/app.py', formatted.replace('chosen(right)', 'right'))
            outer, limited = diff(e, f, 1)
            assert limited['truncated'] and outer['truncated'], outer
            assert all(len(limited[k]) <= 1 for k in ('nodes_changed', 'edges_added', 'edges_removed', 'excluded_documents', 'excluded_functions')), limited
            report = dict(status='passed', checks=['formatting_comment_and_NFKC_equivalence', 'source_offset_stability', 'import_only_call_target_redirect', 'positional_call_and_return_edges', 'literal_expression_edit', 'unsupported_function_coverage_guard', 'unverified_boundary_disclosure', 'immutable_repeated_diff', 'project_isolation', 'bounded_collections_and_outer_truncation', 'no_body_or_semantic_proof_claim','capability_contract'],
                          versions=mcp.call('index_status', {})['versions'], production_database_touched=False, Python_sources_executed=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report))


if __name__ == '__main__':
    main()
