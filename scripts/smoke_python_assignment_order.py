#!/usr/bin/env python3
"""Read-only code graph checks via an isolated MCP database; never runs fixtures."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-assignment-order-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        app=(
            'class Old:\n    def work(self):\n        return 1\n'
            'class New:\n    def __init__(self, value):\n        pass\n    def work(self):\n        return 2\n'
            'class Other:\n    def __init__(self, value):\n        pass\n    def work(self):\n        return 3\n'
            'def module_fn():\n    return 1\ndef class_fn():\n    return 2\ndef global_fn():\n    return 3\n'
            'module_fn = Other(module_fn())\nclass C:\n    class_fn = Other(class_fn())\n'
            'def entry(receiver:Old):\n    receiver = New(receiver.work())\n    receiver.work()\n'
            'def first_assignment():\n    value = New(value.work())\n    value.work()\n'
            'def annotated():\n    global_fn:New\n    return global_fn()\n'
            'def retained(value:Old):\n    value:New\n    return value.work()\n'
            'def augmented(value:Old):\n    value += New(value.work())\n    value.work()\n'
            'def chained(receiver:Old):\n    other = 𝒓eceiver = New(receiver.work())\n    receiver.work()\n'
        )
        for name,body in {'alpha/app.py':app,'beta/app.py':'class Old:\n    def work(self):\n        return 9\n'}.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def calls(name,expr):return [e for e in mcp.call('callees',dict(symbol=items[name]['id']))['items'] if e['reference']['expression']==expr]
            def targets(edges):return [{next(s['qualified_name'] for s in items.values() if s['id']==id) for id in e['candidates']} for e in edges]
            expected=[{'Old::work'},{'New::work'}]
            entry=calls('entry','receiver.work');assert targets(entry)==expected,entry
            first=calls('first_assignment','value.work');assert targets(first)==[set(),{'New::work'}],first
            assert not any(e['candidates'] for e in mcp.call('callees',dict(symbol=items['annotated']['id']))['items'])
            assert targets(calls('retained','value.work'))==[{'Old::work'}]
            augmented=calls('augmented','value.work');assert targets(augmented)==[{'Old::work'},set()],augmented
            assert targets(calls('chained','receiver.work'))==expected
            for name in ('module_fn','class_fn'):
                refs=mcp.call('references',dict(symbol=items[name]['id'],project='alpha'))
                assert any(r['is_call'] and r['expression']==name for r in refs['items']),refs
            for edge in entry:
                ref=edge['reference'];raw=Path(ref['path']).read_bytes();assert raw[ref['start_byte']:ref['start_byte']+len(ref['expression'].encode())].decode()==ref['expression'],ref
            impact=mcp.call('impact',dict(symbol=items['Old::work']['id'],impact_project='alpha'))
            assert items['entry']['id'] in {r['symbol_id'] for r in impact['items']},impact
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert calls('entry','receiver.work')==entry
            assert mcp.call('index_code',dict(force_full_resolution=True))['resolution']['equivalent_to_existing']
            status=mcp.call('index_status',{});assert status['capabilities']['python_binding_evaluation']['rhs_before_write'] and not status['capabilities']['python_binding_evaluation']['compiler_verified'],status
            path=source/'alpha/app.py';path.write_text(app.replace('def entry(receiver:Old)','def entry(receiver:Other)'))
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'));assert not mcp.call('symbol',dict(path=str(path)))['items']
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            assert targets(calls('entry','receiver.work'))==[{'Other::work'},{'New::work'}]
            report=dict(status='passed',checks=['rhs_uses_preceding_type','new_type_after_assignment','first_assignment_uninitialized_guard','annotation_only_local_scope','annotation_retains_existing_value','augmented_result_type_unknown','chained_NFKC_commit','module_and_class_rhs_global','source_bytes','impact_consistency','project_isolation','warm_and_full_resolution_equivalence','capability_scope','stale_exclusion','source_sync_type_refresh'],versions=mcp.call('index_status',{})['versions'],production_database_touched=False,Python_sources_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
