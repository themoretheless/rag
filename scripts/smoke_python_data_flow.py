#!/usr/bin/env python3
"""Real MCP value slices over isolated Python source; never executes that source."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-python-data-flow-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        fixtures={
            'alpha/lib.py':'def identity(value):\n    return value\ndef constant(unused):\n    return 7\n',
            'alpha/app.py':'from lib import identity as chosen\nfrom lib import constant\ndef left(alpha):\n    result = chosen(alpha)\n    return result\ndef right(beta):\n    result = chosen(beta)\n    return result\ndef fixed(input):\n    return constant(input)\ndef versions(first, second):\n    café = first\n    café = second + 1\n    return café\ndef branching(x):\n    if x:\n        return x\n    return 0\ndef formatted(x):\n    f"{mutate()}"\n    return x\ndef normalized(x):\n    𝒙 = 7\n    return x\nasync\tdef deferred(x):\n    return x\n',
            'beta/lib.py':'def identity(foreign):\n    return foreign\ndef left(foreign):\n    return identity(foreign)\n',
        }
        fixtures['alpha/app.py'] += (
            'def merge(flag, left_value, right_value):\n    if flag:\n        merged = chosen(left_value)\n    else:\n        merged = chosen(right_value)\n    return merged\n'
            'def inherited(flag, old, new):\n    result = old\n    if flag:\n        result = new\n    return result\n'
            'def partial(flag, value):\n    if flag:\n        result = value\n    return result\n'
            'def early(first, second, dead, middle, final):\n    if first:\n        result = dead\n        return 7\n    elif second:\n        result = middle\n    else:\n        result = final\n    return result\n'
            'def unrelated(flag, value):\n    if flag:\n        unused = value\n    else:\n        unused = 0\n    return 9\n'
            'def loop(value):\n    while value:\n        value = value - 1\n    return value\n'
        )
        fixtures['alpha/app.py'] += (
            'def loop_calls(flag, initial, next_value):\n    result = initial\n    while flag:\n        result = chosen(next_value)\n        flag = 0\n    return result\n'
            'def iteration(values, initial):\n    result = initial\n    for item in values:\n        result = chosen(item)\n    return result\n'
            'def jumps(flag, stop, skip, initial, broken, continued, normal):\n    result = initial\n    while flag:\n        if stop:\n            result = broken\n            break\n        if skip:\n            result = continued\n            continue\n        result = 7\n    else:\n        result = normal\n    return result\n'
            'def uninitialized_loop(values):\n    for item in values:\n        pass\n    return item\n'
            'def iteration_once(values, replacement, initial):\n    result = initial\n    for item in values:\n        values = replacement\n        result = item\n    return result\n'
            'def loop_continue(flag, initial, following):\n    result = initial\n    while flag:\n        result = following\n        continue\n    return result\n'
            'def exceptions(value):\n    try:\n        return value\n    except build_type():\n        return 0\n'
        )
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            tools=mcp.request('tools/list',{})['tools'];assert any(t['name']=='data_slice' for t in tools),[t['name'] for t in tools]
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def flow(name,**kw):return mcp.call('data_slice',dict(symbol=items[name]['id'],project='alpha',**kw))
            def names(result):return {n['node']['name'] for n in result['items'] if n['node']['name'] is not None}
            def validate(result):
                assert result['status']=='data_slice' and result['capture_format']==7 and not result['runtime_path_proven'] and not result['absence_proven'] and not result['full_program_slice'],result
                ids={n['node']['id'] for n in result['items']}
                for item in result['items']:
                    node=item['node'];origin=item['source'];assert origin['project']=='alpha',item
                    raw=Path(origin['path']).read_bytes();token=raw[node['start_byte']:node['end_byte']].decode('utf-8')
                    if node['name'] is not None:assert token==node['name'],item
                    witness=item['witness'];assert witness['complete_to_seed'] and witness['seed_node_id'] in result['seeds'] and witness['call_contexts_matched'] and not witness['runtime_path_proven'],item
                    at=witness['seed_node_id'];stack=[]
                    for step in witness['steps']:
                        current,target=(step['target'],step['source']) if step['reversed'] else (step['source'],step['target'])
                        assert at==current and stack==step['from_stack'],item
                        frame=step['call_context']
                        if frame:
                            enter=step['relation']=='call_argument' and not step['reversed'] or step['relation']=='return_to_call' and step['reversed']
                            if enter:stack=stack+[frame]
                            elif stack:assert stack[-1]==frame,item;stack=stack[:-1]
                        assert stack==step['to_stack'],item
                        at=target
                    assert at==node['id'] and stack==item['call_stack'],item
                assert all(e['source'] in ids and e['target'] in ids for e in result['edges']),result
                assert len(result['edges'])<=result['limits']['edges']==2000,result
                for edge in result['edges']:
                    transition=edge['transition'];stack=transition['from_stack'][:];frame=edge['call_context']
                    if frame:
                        enter=edge['relation']=='call_argument' and not transition['reversed'] or edge['relation']=='return_to_call' and transition['reversed']
                        if enter:stack.append(frame)
                        elif stack:assert stack[-1]==frame,edge;stack.pop()
                    assert stack==transition['to_stack'] and transition['source_state_reachable'] and not transition['runtime_path_proven'],edge

            backward=flow('left');validate(backward);assert {'alpha','value','result'}<=names(backward) and 'beta' not in names(backward) and 'foreign' not in names(backward),backward
            assert {'call_argument','return_to_call','def_use','assignment','return_value'}<={e['relation'] for e in backward['edges']},backward
            forward=flow('left',direction='forward');validate(forward);assert 'beta' not in names(forward),forward
            assert not any(n['node']['function']==items['right']['id'] for n in forward['items']),forward
            both=flow('left',direction='both');validate(both);assert 'beta' not in names(both),both
            fixed=flow('fixed');validate(fixed);assert 'input' not in names(fixed) and 'unused' not in names(fixed),fixed
            versions=flow('versions');validate(versions);assert 'second' in names(versions) and 'first' not in names(versions),versions
            normalized=flow('normalized');validate(normalized);assert not any(n['node']['kind']=='parameter' for n in normalized['items']),normalized
            assert any(n['node']['name']=='𝒙' and n['node']['binding_name']=='x' for n in normalized['items']),normalized
            ambiguous=flow('versions',variable='café');assert ambiguous['status']=='ambiguous_local_definition',ambiguous
            position=fixtures['alpha/app.py'].encode().rfind('café ='.encode());selected=flow('versions',variable='café',start_byte=position);validate(selected)
            branching=flow('branching');validate(branching);assert branching['branch_control_dependencies'] and any(e['relation']=='control_dependency' for e in branching['edges']),branching
            merged=flow('merge');validate(merged);assert {'flag','left_value','right_value'}<=names(merged),merged
            assert any(e['relation']=='merge_input' for e in merged['edges']),merged
            inherited=flow('inherited');validate(inherited);assert {'flag','old','new'}<=names(inherited),inherited
            early=flow('early');validate(early);assert {'first','second','middle','final'}<=names(early) and 'dead' not in names(early),early
            unrelated=flow('unrelated');validate(unrelated);assert not names(unrelated),unrelated
            partial=flow('partial');assert partial['status']=='unsupported_function' and 'unbound' in partial['reason'],partial
            loop=flow('loop');validate(loop);assert loop['loop_carried_dependencies'] and any(e['relation']=='loop_backedge' for e in loop['edges']),loop
            for budget in (1,10,50,200):
                bounded=flow('merge',max_work=budget);assert bounded['work']<=budget,bounded
            deferred=flow('deferred');assert deferred['status']=='unsupported_function' and 'async' in deferred['reason'],deferred
            formatted=flow('formatted');assert formatted['status']=='unsupported_function' and 'formatted_string' in formatted['reason'],formatted
            loop_calls=flow('loop_calls');validate(loop_calls);assert {'initial','next_value','flag','value'}<=names(loop_calls),loop_calls
            iteration=flow('iteration');validate(iteration);assert {'initial','values','item','value'}<=names(iteration),iteration
            assert any(e['relation']=='iteration_value' for e in iteration['edges']),iteration
            jumps=flow('jumps',depth=30,limit=500);validate(jumps);assert {'broken','normal','stop','flag'}<=names(jumps) and not {'initial','continued'}&names(jumps),jumps
            loop_continue=flow('loop_continue');validate(loop_continue);assert {'flag','initial','following'}<=names(loop_continue),loop_continue
            uninitialized_loop=flow('uninitialized_loop');assert uninitialized_loop['status']=='unsupported_function' and 'unbound' in uninitialized_loop['reason'],uninitialized_loop
            iteration_once=flow('iteration_once');validate(iteration_once);assert 'replacement' not in names(iteration_once),iteration_once
            exceptions=flow('exceptions');assert exceptions['status']=='unsupported_function' and 'dynamic_exception_filter' in exceptions['reason'],exceptions
            for budget in (1,10,50,200,1000):
                bounded=flow('jumps',max_work=budget,depth=30);assert bounded['work']<=budget,bounded
            for budget in (1,5,10,30,100,300):
                bounded=flow('left',max_work=budget);assert bounded['work']<=budget,bounded
            assert flow('left',limit=1)['result_limit_reached']
            assert flow('left',depth=1)['depth_limit_reached']
            ambiguity=mcp.call('data_slice',dict(symbol='left'));assert ambiguity['status']=='ambiguous_function',ambiguity
            structural=mcp.call('slice',dict(symbol=items['left']['id'],direction='forward'));assert structural['detail']['structural_only'] and not backward['structural_call_slice'],structural
            status=mcp.call('index_status',dict(project='alpha'));assert status['data_flow_slicing'] and status['capabilities']['data_flow_slicing'] and status['capabilities']['data_slice']['available'] and not status['capabilities']['cpg']['available'],status
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert flow('left')==backward
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            before=mcp.call('index_status',{})['index_rev'];path=source/'alpha/lib.py';path.write_text(fixtures['alpha/lib.py'].replace('return value','return 9'))
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'))
            stale=flow('left');assert stale['stale_documents'] and stale['call_boundary_diagnostics_total']>0 and 'value' not in names(stale),stale
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            refreshed=flow('left');validate(refreshed);assert 'alpha' not in names(refreshed) and 'value' not in names(refreshed),refreshed
            after=mcp.call('index_status',{})['index_rev'];delta=mcp.call('diff',dict(before=before,after=after,project='alpha'));assert delta['data_flow_comparison']['available'] and delta['data_flow_comparison']['nodes_changed_total']>0 and delta['data_flow_comparison']['before_capture_available'] and delta['data_flow_comparison']['after_capture_available'],delta
            report=dict(status='passed',versions=mcp.call('index_status',{})['versions'],checks=['tool_registered','local_def_use_latest_write','positional_argument_and_return_edges','call_site_context_isolation','forward_back_both','constant_return_no_argument_influence','source_UTF8_spans','NFKC_binding_identity','ambiguous_definition_guard','branch_returns_and_control_dependencies','branch_merge_with_call_frames','no_else_reaching_values','early_return_exclusion','unrelated_condition_exclusion','partial_initialization_guard','loop_carried_values','while_calls','for_iteration_calls','break_continue_else','continue_backedge','matched_edge_contexts_and_cycles','loop_zero_path_guard','iterable_evaluated_once','dynamic_exception_filter_guard','formatted_string_guard','async_keyword_guard','project_isolation','work_depth_node_limits','structural_slice_separation','capability_scope','warm_equivalence','full_resolution_equivalence','stale_callee_boundary','source_sync_value_change','frozen_normalized_value_diff'],Python_sources_executed=False,production_database_touched=False,runtime_path_proven=False,full_program_slice=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
