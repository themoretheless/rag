#!/usr/bin/env python3
"""Bounded receiver-origin joins through real MCP; text-only temporary fixtures."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-receiver-joins-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        app=(
            'from lib import Base, Derived\n'
            'class Old:\n    def work(self):\n        return 1\nclass New:\n    def work(self):\n        return 2\nclass Third:\n    def work(self):\n        return 3\nclass Fourth:\n    def work(self):\n        return 4\n'
            'def optional(receiver:Old, flag):\n    if flag:\n        receiver = New()\n    receiver.work()\n'
            'def both(receiver:Old, flag):\n    if flag:\n        receiver = New()\n    else:\n        receiver = Third()\n    receiver.work()\n'
            'def returned(receiver:Old, flag):\n    if flag:\n        receiver = New()\n        return 0\n    receiver.work()\n'
            'def loop(receiver:Old, flag):\n    while flag:\n        receiver.work()\n        receiver = New()\n    receiver.work()\n'
            'def iteration(receiver:Old, values):\n    for item in values:\n        receiver = New()\n    receiver.work()\n'
            'def alias(receiver:Old, flag):\n    𝒂lias = receiver\n    if flag:\n        alias = New()\n    alias.work()\n'
            'def partial(receiver:Old, flag, unknown):\n    if flag:\n        receiver = unknown\n    receiver.work()\n'
            'def inherited(receiver:Base, flag):\n    if flag:\n        receiver = Derived()\n    receiver.work()\n'
        )
        fixtures={'alpha/app.py':app,'alpha/lib.py':'class Base:\n    def work(self):\n        return 7\nclass Derived(Base):\n    pass\n','beta/app.py':'class Old:\n    def work(self):\n        return 9\n'}
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def calls(name):return [e for e in mcp.call('callees',dict(symbol=items[name]['id']))['items'] if e['reference']['name']=='work']
            def targets(edge):return {next(s['qualified_name'] for s in items.values() if s['id']==id) for id in edge['candidates']}
            optional=calls('optional');assert len(optional)==1 and targets(optional[0])=={'Old::work','New::work'} and optional[0]['resolution']=='ambiguous',optional
            assert targets(calls('both')[0])=={'New::work','Third::work'}
            assert targets(calls('returned')[0])=={'Old::work'}
            loop=calls('loop');assert len(loop)==2 and all(targets(e)=={'Old::work','New::work'} for e in loop),loop
            assert targets(calls('iteration')[0])=={'Old::work','New::work'}
            assert targets(calls('alias')[0])=={'Old::work','New::work'}
            partial=calls('partial')[0];assert targets(partial)=={'Old::work'} and partial['resolution']=='ambiguous' and partial['resolution_basis']=='python_receiver_type_join_with_unknown',partial
            inherited=calls('inherited')[0];assert targets(inherited)=={'Base::work'} and inherited['resolution']=='resolved_static',inherited
            dead=mcp.call('dead_code',dict(project='alpha',limit=500));assert not any(i['symbol']['qualified_name']=='Fourth::work' for i in dead['items']),dead
            for name in ('Old::work','New::work'):
                impact=mcp.call('impact',dict(symbol=items[name]['id'],impact_project='alpha'))
                assert items['optional']['id'] in {i['symbol_id'] for i in impact['items']},impact
            for edge in optional:
                ref=edge['reference'];raw=Path(ref['path']).read_bytes();assert raw[ref['start_byte']:ref['start_byte']+len(ref['expression'].encode())].decode()==ref['expression'],ref
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert calls('optional')==optional
            assert mcp.call('index_code',dict(force_full_resolution=True))['resolution']['equivalent_to_existing']
            status=mcp.call('index_status',{})['capabilities']['python_binding_evaluation'];assert status['receiver_origin_joins'] and not status['compiler_verified'],status
            path=source/'alpha/app.py';path.write_text(app.replace('receiver = New()', 'receiver = Third()',1))
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'));assert not mcp.call('symbol',dict(path=str(path)))['items']
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols();assert targets(calls('optional')[0])=={'Old::work','Third::work'}
            report=dict(status='passed',checks=['optional_branch_union','both_arms_kill_old_type','returned_arm_exclusion','loop_fixed_point_and_zero_path','for_zero_path','NFKC_local_alias','partial_known_target_not_certain','imported_inherited_target_dedup','dead_code_unknown_root_guard','impact_both_targets','source_bytes','project_isolation','warm_and_full_resolution_equivalence','capability_scope','stale_exclusion','source_sync_join_change'],versions=mcp.call('index_status',{})['versions'],production_database_touched=False,Python_sources_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
