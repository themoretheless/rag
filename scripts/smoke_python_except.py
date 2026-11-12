#!/usr/bin/env python3
"""Isolated real MCP handler/else/finally candidate routes; no source execution."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-except-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        app=(
            'from lib import identity\n'
            'def join(initial, numerator, denominator, fallback):\n    value = initial\n    try:\n        value = numerator / denominator\n    except:\n        value = fallback\n    return value\n'
            'def candidates(problem, left, right, fallback):\n    try:\n        raise problem\n    except ValueError:\n        return identity(left)\n    except TypeError:\n        return identity(right)\n    except:\n        return fallback\n'
            'def alias(problem):\n    try:\n        raise problem\n    except Exception as 𝒆rr:\n        return err\n'
            'def cleanup(problem, err):\n    try:\n        raise problem\n    except Exception as err:\n        pass\n    return err\n'
            'def reraised(problem, result):\n    try:\n        try:\n            raise problem\n        except Exception as err:\n            raise\n    except:\n        return result\n'
            'def else_paths(numerator, denominator, handled, normal):\n    try:\n        value = numerator / denominator\n    except:\n        value = handled\n    else:\n        value = normal\n    return value\n'
            'def else_escape(problem, wrong, result):\n    try:\n        value = 1\n    except:\n        return wrong\n    else:\n        raise problem\n    finally:\n        return result\n'
            'def handler_escape(original, replacement):\n    try:\n        raise original\n    except:\n        raise replacement\n'
            'def final_override(problem, result):\n    try:\n        raise problem\n    except Exception as err:\n        return err\n    finally:\n        return result\n'
            'def alias_calls(problem):\n    try:\n        raise problem\n    except Exception as 𝒊dentity:\n        identity(problem)\n    return identity(problem)\n'
            'def plain_calls(problem, x):\n    try:\n        raise problem\n    except Exception as err:\n        return identity(x)\n'
            'def dynamic_selector(x):\n    try:\n        return x\n    except build_type():\n        return 0\n'
        )
        fixtures={'alpha/app.py':app,'alpha/lib.py':'def identity(value):\n    return value\n','beta/app.py':'def candidates(foreign):\n    return foreign\n'}
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def flow(name,**kwargs):return mcp.call('data_slice',dict(symbol=items[name]['id'],project='alpha',**kwargs))
            def names(value):return {n['node']['name'] for n in value['items'] if n['node']['name'] is not None}
            def validate(value):
                assert value['status']=='data_slice' and value['capture_format']==7 and value['exception_handler_paths'],value
                assert not value['exception_type_matching_proven'] and not value['exception_filter_evaluation_proven'] and not value['runtime_path_proven'] and not value['absence_proven'],value
                ids={n['node']['id'] for n in value['items']};assert len(ids)==len(value['items']),value
                for item in value['items']:
                    node=item['node'];assert item['source']['project']=='alpha',item
                    if node['name'] is not None:
                        raw=Path(item['source']['path']).read_bytes();assert raw[node['start_byte']:node['end_byte']].decode()==node['name'],item
                    assert item['witness']['complete_to_seed'] and item['witness']['call_contexts_matched'],item
                assert all(e['source'] in ids and e['target'] in ids for e in value['edges']),value
            joined=flow('join');validate(joined);assert {'numerator','denominator','fallback'}<=names(joined) and 'initial' not in names(joined),joined
            assert any(e['relation']=='try_merge_input' for e in joined['edges']),joined
            candidates=flow('candidates');validate(candidates);assert {'left','right','fallback'}<=names(candidates),candidates
            assert len({e['call_context'] for e in candidates['edges'] if e['relation']=='call_argument'})>=2,candidates
            alias=flow('alias');validate(alias);assert {'problem','𝒆rr','err'}<=names(alias),alias
            assert any(e['relation']=='exception_binding' for e in alias['edges']),alias
            assert flow('cleanup')['status']=='unsupported_function'
            reraised=flow('reraised');validate(reraised);assert 'result' in names(reraised),reraised
            assert sum(n['node']['kind']=='raise' for n in reraised['items'])>=2,reraised
            else_paths=flow('else_paths');validate(else_paths);assert {'handled','normal'}<=names(else_paths),else_paths
            escaped=flow('else_escape');validate(escaped);assert 'result' in names(escaped) and not {'wrong','problem'}&names(escaped),escaped
            assert flow('handler_escape')['status']=='seed_unavailable'
            overridden=flow('final_override');validate(overridden);assert 'result' in names(overridden) and 'problem' not in names(overridden),overridden
            calls=mcp.call('callees',dict(symbol=items['alias_calls']['id']));assert len(calls['items'])==2 and all(not e['candidates'] for e in calls['items']),calls
            plain=flow('plain_calls');validate(plain);assert {'x','value'}<=names(plain),plain
            assert flow('dynamic_selector')['status']=='unsupported_function'
            for budget in (1,10,50,200,1000):assert flow('candidates',max_work=budget)['work']<=budget
            assert flow('join',limit=1)['result_limit_reached']
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert flow('candidates')==candidates
            assert mcp.call('index_code',dict(force_full_resolution=True))['resolution']['equivalent_to_existing']
            before=mcp.call('index_status',{})['index_rev'];path=source/'alpha/app.py';path.write_text(app.replace('identity(left)','identity(right)',1))
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'));assert flow('candidates')['status']!='data_slice'
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            refreshed=flow('candidates');validate(refreshed);assert 'right' in names(refreshed) and 'left' not in names(refreshed),refreshed
            after=mcp.call('index_status',{})['index_rev'];diff=mcp.call('diff',dict(before=before,after=after,project='alpha'))['data_flow_comparison']
            assert diff['available'] and diff['normalization_format']==3 and diff['nodes_changed_total']>0,diff
            report=dict(status='passed',checks=['bare_handler_reaching_definitions','normal_and_handled_merge_edges','typed_candidates_unproven','separate_handler_call_frames','caught_alias_UTF8_value','alias_cleanup_guard','captured_bare_reraise','try_else_normal_only','else_error_not_caught_by_siblings','handler_error_escapes','finally_return_override','core_alias_shadow_and_cleanup','unrelated_import_call_resolution','dynamic_selector_guard','work_and_output_limits','project_isolation','warm_and_full_resolution_equivalence','stale_exclusion','source_sync_handler_change','frozen_handler_context_diff'],versions=mcp.call('index_status',{})['versions'],production_database_touched=False,Python_sources_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
