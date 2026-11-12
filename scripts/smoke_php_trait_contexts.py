#!/usr/bin/env python3
"""Qualify consumer-specific PHP trait body candidates over isolated real MCP."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-trait-contexts-') as temp:
        root=Path(temp).resolve()
        source=root/'sources'
        fixtures={
            'alpha/trait.php': '<?php namespace Lib; trait T {public function run(){self::save();parent::load();$this->privateOwn();$f=self::save(...);static::save();}}',
            'alpha/hosts.php': '<?php namespace App; use Lib\\T; class P1 {public static function save(){} public static function load(){}} class P2 {public static function save(){} public static function load(){}} class A extends P1 {use T;private function privateOwn(){} public function entry(){$this->run();}} class B extends P2 {use T;private function privateOwn(){} public function entry(){$this->run();}}',
            'beta/foreign.php': '<?php namespace Lib; trait T {public function run(){self::save();}} namespace App; class Foreign {use \\Lib\\T;public static function save(){}}',
        }
        for name,body in fixtures.items():
            path=source/name
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                sync=mcp.call('sync_sources',dict(path=str(source/project),wing=project))
                assert not sync.get('errors'),sync
            def symbols():
                result=mcp.call('symbol',dict(project='alpha',limit=500))
                assert not result.get('parse_errors'),result
                return {s['qualified_name']:s for s in result['items']}
            items=symbols()
            def calls():return mcp.call('callees',dict(symbol=items['Lib\\T::run']['id']))
            def contexts(edge):
                evidence=edge['context_evidence']
                assert evidence['kind']=='php_captured_trait_consumers' and evidence['enumeration_complete'],edge
                assert not evidence['runtime_contexts_complete'] and not evidence['context_correlation_proven'],edge
                assert not evidence['truncated'],edge
                names={s['id']:name for name,s in items.items()}
                return {names[c['owner']]:[names[t] for t in c['candidates']] for c in evidence['contexts']}
            initial=calls()
            by_expr={e['reference']['expression']:e for e in initial['items']}
            assert len(initial['items'])==4,initial
            assert contexts(by_expr['self::save'])=={'App\\A':['App\\P1::save'],'App\\B':['App\\P2::save']},initial
            assert contexts(by_expr['parent::load'])=={'App\\A':['App\\P1::load'],'App\\B':['App\\P2::load']},initial
            assert contexts(by_expr['$this->privateOwn'])=={'App\\A':['App\\A::privateOwn'],'App\\B':['App\\B::privateOwn']},initial
            assert all(e['resolution']=='ambiguous' for e in initial['items'] if e['candidates']),initial
            assert not by_expr['static::save']['candidates'] and all(c['resolution_basis']=='PHP_LATE_STATIC_BINDING_UNVERIFIED' for c in by_expr['static::save']['context_evidence']['contexts'])
            refs=mcp.call('references',dict(symbol=items['App\\P1::save']['id'],project='alpha'))
            assert any(not r['is_call'] and r['context_evidence']['contexts'] for r in refs['items']),refs
            callers=mcp.call('callers',dict(symbol=items['App\\P1::save']['id']))
            assert len(callers['items'])==1 and callers['items'][0]['context_evidence']==by_expr['self::save']['context_evidence'],callers
            impact=mcp.call('impact',dict(symbol=items['App\\P1::save']['id'],impact_project='alpha'))
            assert not impact['runtime_impact_proven'],impact
            row=next(r for r in impact['items'] if r['symbol_id']==items['Lib\\T::run']['id'])
            assert row['via']['context_evidence']['context_correlation_proven'] is False,row
            warm=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            assert warm['code_index']['indexed']==0 and calls()==initial,warm
            full=mcp.call('index_code',dict(force_full_resolution=True))
            assert not full['revision_created'] and full['resolution']['equivalent_to_existing'] is True,full
            before=mcp.call('index_status',dict(project='alpha'))['index_rev']
            hosts=source/'alpha/hosts.php'
            changed=fixtures['alpha/hosts.php'].replace('A extends P1','A extends P2').replace('B extends P2','B extends P1')
            hosts.write_text(changed)
            mcp.call('ingest_file',dict(path=str(hosts),wing='alpha'))
            stale=calls()
            assert stale['stale_documents'] and all(not e['candidates'] for e in stale['items']),stale
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            updated={e['reference']['expression']:e for e in calls()['items']}
            assert contexts(updated['self::save'])=={'App\\A':['App\\P2::save'],'App\\B':['App\\P1::save']},updated
            after=mcp.call('index_status',dict(project='alpha'))['index_rev']
            delta=mcp.call('diff',dict(before=before,after=after,project='alpha'))
            assert delta['edges_added_total']==0 and delta['edges_removed_total']==0,delta
            assert delta['php_trait_context_comparison']['available'],delta
            assert not delta['php_trait_context_comparison']['context_correlation_proven'],delta
            assert sum(r['kind']=='php_trait_body_context_candidate' for r in delta['relations_added'])==6,delta
            foreign=mcp.call('diff',dict(before=before,after=after,project='beta'))
            assert not foreign['edges_added'] and not foreign['relations_added'],foreign
            # Remove A's direct trait use; inherited runtime subtype contexts are not invented.
            hosts.write_text(changed.replace('class A extends P2 {use T;','class A extends P2 {'))
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            final={e['reference']['expression']:e for e in calls()['items']}
            assert contexts(final['self::save'])=={'App\\B':['App\\P1::save']},final
            assert final['self::save']['resolution']=='resolved_static',final
            assert mcp.call('diff',dict(before=before,after=after,project='alpha'))==delta
            full=mcp.call('index_code',dict(force_full_resolution=True))
            assert full['resolution']['equivalent_to_existing'] is True,full
            result=dict(passed=True,versions=mcp.call('index_status',{})['versions'],checks=['self-this-parent-per-consumer','private-effective-owner','multiple-context-ambiguity','late-static-guard','callable-value-not-call','flat-reference-context-evidence','callers-and-impact-provenance','project-isolation','warm-reuse','full-cache-equivalence','stale-host-exclusion','owner-target-swap-with-unchanged-union','frozen-context-diff','consumer-removal-refresh','historical-immutability'],runtime_contexts_complete=False,context_correlation_proven=False,production_database_touched=False,source_applications_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))


if __name__=='__main__':main()
