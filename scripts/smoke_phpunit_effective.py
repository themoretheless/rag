#!/usr/bin/env python3
"""Real MCP effective PHPUnit candidates; never executes PHP or PHPUnit."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-phpunit-effective-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        fixtures={
            'alpha/base.php':r'<?php namespace Suite; function work(){} function other(){} function common(){} trait Checks {public function testTrait(){self::leaf();}} abstract class Base extends \PHPUnit\Framework\TestCase {public function testInherited(){work();}}',
            'alpha/hosts.php':r'<?php namespace Suite; class A extends Base {use Checks {testTrait as testAlias;} public static function leaf(){common();}} class B extends Base {use Checks;public static function leaf(){common();}public function testInherited(){other();}}',
            'beta/foreign.php':r'<?php namespace Suite; class A extends \PHPUnit\Framework\TestCase {public function testForeign(){}}',
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def catalog(**kw):return [e for e in mcp.call('entrypoints',dict(project='alpha',limit=500,**kw))['items'] if e['kind']=='phpunit_test_candidate']
            entries=catalog(symbol=items['Suite\\A']['id'],path=str(source/'alpha/hosts.php'))
            assert {e['test_context']['effective_method'] for e in entries}=={'testTrait','testAlias','testInherited'},entries
            assert all(e['test_context']['host_symbol_id']==items['Suite\\A']['id'] for e in entries),entries
            assert all(e['document_id']==items['Suite\\Checks::testTrait']['document_id'] for e in entries),entries
            assert all(e['test_context']['host_document_id']!=e['document_id'] for e in entries),entries
            def affected(name,**kw):return mcp.call('affected_tests',dict(symbols=[items[name]['id']],project='alpha',**kw))
            def cases(result):return {(r['test_host']['qualified_name'],r['effective_test_method']) for r in result['items']}
            def validate(result):
                assert not result['safe_to_skip_unlisted_tests'] and not result['coverage_complete'] and not result['runtime_coverage_measured'],result
                by_id={s['id']:s for s in items.values()}
                for item in result['items']:
                    c=item['entrypoint']['test_context'];host=by_id[c['host_symbol_id']]
                    assert host['id']==item['test_host']['id'] and host['document_id']==c['host_document_id'] and host['content_hash']==c['host_content_hash'],item
                    assert not c['framework_execution_verified'] and not item['commands'],item
                    assert item['command_status']=='no_qualified_command_candidate',item
                    path=item['path'];assert path['complete_to_seed'] and not path['runtime_context_correlation_proven'],item
                    nodes=path['symbols'];steps=path['steps'];assert len(nodes)==len(steps)+1,item
                    for i,step in enumerate(steps):
                        assert step['source_symbol_id']==nodes[i]['id'] and step['target_symbol_id']==nodes[i+1]['id'],item
                        ref=step['evidence']['reference'];assert ref['document_id']==nodes[i]['document_id'] and ref['content_hash']==nodes[i]['content_hash'],item
                        if i:assert steps[i-1]['target_context']==step['source_context'],item
                    if steps and c['trait_body']:assert steps[0]['source_context']==c['lexical_owner_symbol_id'],item
            a=affected('Suite\\A::leaf');assert cases(a)=={('Suite\\A','testTrait'),('Suite\\A','testAlias')},a;validate(a)
            b=affected('Suite\\B::leaf');assert cases(b)=={('Suite\\B','testTrait')},b;validate(b)
            shared=affected('Suite\\common');assert cases(shared)==cases(a)|cases(b),shared;validate(shared)
            inherited=affected('Suite\\work');assert cases(inherited)=={('Suite\\A','testInherited')},inherited;validate(inherited)
            override=affected('Suite\\other');assert cases(override)=={('Suite\\B','testInherited')},override;validate(override)
            direct=affected('Suite\\Checks::testTrait');assert cases(direct)==cases(shared) and all(r['reason']=='changed_test' for r in direct['items']),direct;validate(direct)
            capped=affected('Suite\\common',limit=1);assert len(capped['items'])==1 and capped['related_tests_total_in_examined_graph']==3 and capped['truncated'],capped
            assert shared['invocation_match_work']>0 and not shared['trait_context_traversal']['symbol_witnesses_deduplicated'],shared
            for budget in (1,10,25,50,100):
                result=affected('Suite\\common',max_work=budget);assert result['work']<=budget,result
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert affected('Suite\\common')==shared
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            before=mcp.call('index_status',dict(project='alpha'))['index_rev']
            hosts=source/'alpha/hosts.php';hosts.write_text(fixtures['alpha/hosts.php'].replace('testAlias','testRenamed'))
            mcp.call('ingest_file',dict(path=str(hosts),wing='alpha'))
            stale=affected('Suite\\work');assert not stale['items'] and stale['stale_documents'],stale
            assert not catalog(), 'stale host must not supply inherited/trait invocations'
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            refreshed=affected('Suite\\A::leaf');assert cases(refreshed)=={('Suite\\A','testTrait'),('Suite\\A','testRenamed')},refreshed;validate(refreshed)
            after=mcp.call('index_status',dict(project='alpha'))['index_rev']
            delta=mcp.call('diff',dict(before=before,after=after,project='alpha'));assert delta['phpunit_test_comparison']['available'],delta
            added=[r for r in delta['relations_added'] if r.get('entrypoint_kind')=='phpunit_test_candidate']
            removed=[r for r in delta['relations_removed'] if r.get('entrypoint_kind')=='phpunit_test_candidate']
            assert len(added)==len(removed)==1 and added[0]['test_context']['effective_method']=='testRenamed' and removed[0]['test_context']['effective_method']=='testAlias',delta
            report=dict(status='passed',versions=mcp.call('index_status',{})['versions'],checks=['class_scoped_catalog','inherited_body_host_provenance','trait_alias_invocations','consumer_frame_matching','shared_body_multiple_states','inherited_test','override_precedence','changed_trait_body_all_invocations','independent_invocation_limit','shared_work_budget','warm_equivalence','full_resolution_equivalence','stale_host_exclusion','source_sync_alias_change','normalized_frozen_invocation_diff','project_isolation'],PHP_executed=False,PHPUnit_executed=False,runtime_test_discovery_complete=False,production_database_touched=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
