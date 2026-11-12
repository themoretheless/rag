#!/usr/bin/env python3
"""Real MCP explicit completion/finally paths over text-only isolated sources."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-finally-') as tmp:
        root = Path(tmp).resolve()
        source = root/'sources'
        app = (
            'from lib import identity\n'
            'def saved(first, second):\n    value = first\n    try:\n        return value\n    finally:\n        value = second\n'
            'def override(first, second):\n    try:\n        return first + 1\n    finally:\n        return second\n'
            'def prefix(initial, numerator, denominator):\n    value = initial\n    try:\n        value = numerator / denominator\n    finally:\n        return value\n'
            'def constant(input):\n    try:\n        value = input / 0\n    finally:\n        return 7\n'
            'def nested(first, second):\n    value = first\n    try:\n        try:\n            return value\n        finally:\n            value = second\n    finally:\n        return value\n'
            'def call_contexts(flag, left, right):\n    result = left\n    try:\n        if flag:\n            return 0\n        result = right\n    finally:\n        𝒇inal = identity(result)\n        return final\n'
            'def break_override(flag, discarded, result):\n    while flag:\n        try:\n            return discarded\n        finally:\n            break\n    return result\n'
            'def continue_state(flag, initial, overwritten, replacement):\n    result = initial\n    while flag:\n        try:\n            result = overwritten\n            continue\n        finally:\n            result = replacement\n    return result\n'
            'def raised(problem, result):\n    try:\n        raise problem\n    finally:\n        return result\n'
            'def escaping(problem, discarded):\n    try:\n        return discarded\n    finally:\n        raise problem\n'
            'def handler(x):\n    try:\n        return x\n    except Exception:\n        return 0\n'
            'def uninitialized(x):\n    try:\n        value = x / 0\n    finally:\n        return value\n'
        )
        fixtures = {'alpha/app.py': app, 'alpha/lib.py': 'def identity(value):\n    return value\n',
                    'beta/app.py': 'def override(foreign):\n    return foreign\n'}
        for name, body in fixtures.items():
            path = source/name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ('alpha', 'beta'):
                result = mcp.call('sync_sources', dict(path=str(source/project), wing=project))
                assert not result.get('errors'), result
            def symbols():
                return {s['qualified_name']: s for s in mcp.call('symbol', dict(project='alpha', limit=500))['items']}
            items = symbols()
            def flow(name, **kwargs):
                return mcp.call('data_slice', dict(symbol=items[name]['id'], project='alpha', **kwargs))
            def names(value):
                return {n['node']['name'] for n in value['items'] if n['node']['name'] is not None}
            def validate(value):
                assert value['status'] == 'data_slice' and value['capture_format'] == 7 and value['finally_completion_paths'], value
                assert not value['full_exception_semantics'] and value['exception_handler_paths'] and not value['runtime_path_proven'] and not value['absence_proven'], value
                ids = {n['node']['id'] for n in value['items']}
                assert len(ids) == len(value['items']), value
                for item in value['items']:
                    node = item['node']
                    assert item['source']['project'] == 'alpha', item
                    raw = Path(item['source']['path']).read_bytes()
                    if node['name'] is not None:
                        assert raw[node['start_byte']:node['end_byte']].decode('utf-8') == node['name'], item
                    witness = item['witness']
                    assert witness['complete_to_seed'] and witness['call_contexts_matched'] and not witness['runtime_path_proven'], item
                    at = witness['seed_node_id']
                    stack = []
                    for step in witness['steps']:
                        current, target = (step['target'], step['source']) if step['reversed'] else (step['source'], step['target'])
                        assert at == current and stack == step['from_stack'], item
                        frame = step['call_context']
                        if frame:
                            enter = step['relation'] == 'call_argument' and not step['reversed'] or step['relation'] == 'return_to_call' and step['reversed']
                            if enter:
                                stack.append(frame)
                            elif stack:
                                assert stack[-1] == frame, item
                                stack.pop()
                        assert stack == step['to_stack'], item
                        at = target
                    assert at == node['id'] and stack == item['call_stack'], item
                assert all(e['source'] in ids and e['target'] in ids for e in value['edges']), value
            saved = flow('saved'); validate(saved)
            assert 'first' in names(saved) and 'second' not in names(saved), saved
            overridden = flow('override'); validate(overridden)
            assert 'second' in names(overridden) and 'first' not in names(overridden), overridden
            prefix = flow('prefix'); validate(prefix)
            assert {'initial', 'numerator', 'denominator'} <= names(prefix), prefix
            constant = flow('constant'); validate(constant)
            assert not names(constant), constant
            nested = flow('nested'); validate(nested)
            assert 'second' in names(nested) and 'first' not in names(nested), nested
            contexts = flow('call_contexts'); validate(contexts)
            assert {'left', 'right'} <= names(contexts), contexts
            frames = {e['call_context'] for e in contexts['edges'] if e['relation'] == 'call_argument'}
            assert len(frames) >= 2, contexts
            assert any(n['node'].get('flow_context') for n in contexts['items']), contexts
            selected = flow('call_contexts', variable='final', start_byte=app.encode().find('𝒇inal ='.encode())); validate(selected)
            assert len(selected['seeds']) >= 2, selected
            broken = flow('break_override'); validate(broken)
            assert 'result' in names(broken) and 'discarded' not in names(broken), broken
            continued = flow('continue_state'); validate(continued)
            assert {'initial', 'replacement'} <= names(continued) and 'overwritten' not in names(continued), continued
            raised = flow('raised'); validate(raised)
            assert 'result' in names(raised) and 'problem' not in names(raised), raised
            assert flow('escaping')['status'] == 'seed_unavailable'
            assert flow('handler')['status'] == 'data_slice'
            assert flow('uninitialized')['status'] == 'unsupported_function'
            for budget in (1, 10, 50, 200, 1000):
                bounded = flow('prefix', max_work=budget)
                assert bounded['work'] <= budget, bounded
            assert flow('prefix', limit=1)['result_limit_reached']
            status = mcp.call('index_status', {})
            assert status['capabilities']['data_slice']['finally_completion_paths'] and status['capabilities']['data_slice']['exception_handler_paths'], status
            mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha'))
            assert flow('call_contexts') == contexts
            assert mcp.call('index_code', dict(force_full_resolution=True))['resolution']['equivalent_to_existing']
            before = mcp.call('index_status', {})['index_rev']
            path = source/'alpha/app.py'
            path.write_text(app.replace('finally:\n        return second', 'finally:\n        return 9', 1))
            mcp.call('ingest_file', dict(path=str(path), wing='alpha'))
            assert flow('override')['status'] != 'data_slice'
            mcp.call('sync_sources', dict(path=str(source/'alpha'), wing='alpha')); items = symbols()
            refreshed = flow('override'); validate(refreshed)
            assert not names(refreshed), refreshed
            after = mcp.call('index_status', {})['index_rev']
            diff = mcp.call('diff', dict(before=before, after=after, project='alpha'))['data_flow_comparison']
            assert diff['available'] and diff['normalization_format'] == 3 and diff['nodes_changed_total'] > 0, diff
            report = dict(status='passed', checks=['saved_return_value', 'finally_return_override', 'pre_assignment_exception_prefix', 'constant_return_independence', 'nested_finally_local_updates', 'same_call_multiple_completion_frames', 'UTF8_occurrence_evidence', 'one_definition_all_context_seeds', 'break_override', 'continue_updated_state', 'raise_override', 'escaping_raise_no_return_seed', 'handler_availability_and_uninitialized_guard', 'work_and_output_limits', 'capability_limits', 'project_isolation', 'warm_and_full_resolution_equivalence', 'stale_exclusion', 'source_sync_finally_change', 'frozen_context_diff'], versions=mcp.call('index_status', {})['versions'], production_database_touched=False, Python_sources_executed=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+'\n')
            print(json.dumps(report))


if __name__ == '__main__':
    main()
