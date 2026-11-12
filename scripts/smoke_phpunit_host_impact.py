#!/usr/bin/env python3
"""Real MCP class/trait test availability and alias-only Git changes."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-phpunit-host-impact-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        fixtures={
            'alpha/traits.php':'<?php trait Tests {public function testWorks(){self::leaf();}} trait Outer {use Tests;}',
            'alpha/base.php':r'<?php abstract class Base extends \PHPUnit\Framework\TestCase {}',
            'alpha/a.php':'<?php class A extends Base {use Outer {testWorks as testAlias;}public static function leaf(){}}',
            'alpha/b.php':'<?php class B extends Base {use Tests;public static function leaf(){}}',
            'beta/foreign.php':r'<?php class A extends \PHPUnit\Framework\TestCase {public function testForeign(){}}',
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        repository=source/'alpha'
        def git(*args):
            return subprocess.run(['git','-C',str(repository),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE).stdout
        git('init','-q');git('add','.');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','base')
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def affected(name,**kw):return mcp.call('affected_tests',dict(symbols=[items[name]['id']],project='alpha',**kw))
            def cases(result):return {(r['test_host']['qualified_name'],r['effective_test_method']) for r in result['items']}
            def validate(result):
                assert not result['safe_to_skip_unlisted_tests'] and not result['coverage_complete'] and not result['runtime_coverage_measured'],result
                for item in result['items']:
                    path=item['path'];nodes=path['symbols'];steps=path['steps']
                    assert path['complete_to_seed'] and len(nodes)==len(steps)+1 and item['seed_symbol_id']==nodes[-1]['id'],item
                    assert item['reason']=='captured_test_host_dependency' and not item['commands'],item
                    assert not path['runtime_context_correlation_proven'],item
                    assert steps[0]['relation']=='phpunit_host_candidate',item
                    for i,step in enumerate(steps):
                        assert step['source_symbol_id']==nodes[i]['id'] and step['target_symbol_id']==nodes[i+1]['id'],item
                        if i:assert steps[i-1]['target_context']==step['source_context'],item
                        evidence=step['evidence'];assert 'reference' not in evidence,step
                        if step['relation']=='phpunit_host_candidate':
                            assert not evidence['call_edge'] and not evidence['runtime_registration_verified'],step
                            assert evidence['body_document_id']==nodes[i]['document_id'] and evidence['body_content_hash']==nodes[i]['content_hash'],step
                            assert evidence['host_symbol_id']==nodes[i+1]['id'] and evidence['host_content_hash']==nodes[i+1]['content_hash'],step
                        else:
                            assert step['relation']=='inheritance_candidate',step
                            provenance=evidence['source_provenance'];assert provenance['document_id']==nodes[i]['document_id'] and provenance['content_hash']==nodes[i]['content_hash'] and evidence['path']==nodes[i]['path'],step
            a=affected('A');assert cases(a)=={('A','testWorks'),('A','testAlias')},a;validate(a)
            assert all(len(i['path']['steps'])==1 for i in a['items']),a
            assert not affected('A',include_host_dependencies=False)['items']
            for mode in ('calls','references','all'):
                scoped=affected('A',relations=mode);assert cases(scoped)==cases(a),scoped;validate(scoped)
            base=affected('Base');assert cases(base)==cases(a)|{('B','testWorks')},base;validate(base)
            trait=affected('Tests');assert cases(trait)==cases(base),trait;validate(trait)
            shallow=affected('Tests',depth=2);assert cases(shallow)=={('B','testWorks')} and shallow['depth_limited'],shallow;validate(shallow)
            capped=affected('Base',limit=1);assert capped['related_tests_total_in_examined_graph']==3 and capped['truncated'] and len(capped['items'])==1,capped
            for budget in (1,5,10,20,50,100):
                result=affected('Base',max_work=budget);assert result['work']<=budget,result
            direct=affected('A',max_work=1);assert cases(direct)==cases(a) and direct['direct_test_host_matches']==2,direct;validate(direct)
            mcp.call('sync_sources',dict(path=str(repository),wing='alpha'));assert affected('A')==a
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            a_path=repository/'a.php';a_path.write_text(fixtures['alpha/a.php'].replace('testAlias','testRenamed'))
            index_bytes=(repository/'.git/index').read_bytes();index_rev=mcp.call('index_status',{})['index_rev']
            changed=mcp.call('detect_changes',dict(repository=str(repository),project='alpha',scope='all',include_tests=True,depth=10))
            before=changed['before_impact']['affected_tests'];after=changed['after_impact']['affected_tests']
            assert cases(before)=={('A','testWorks'),('A','testAlias')},changed
            assert cases(after)=={('A','testWorks'),('A','testRenamed')},changed
            validate(before);validate(after)
            assert not changed['persistent_index_used'] and (repository/'.git/index').read_bytes()==index_bytes,changed
            assert mcp.call('index_status',{})['index_rev']==index_rev
            mcp.call('ingest_file',dict(path=str(a_path),wing='alpha'))
            stale=affected('Tests');assert cases(stale)=={('B','testWorks')} and stale['stale_documents'],stale;validate(stale)
            mcp.call('sync_sources',dict(path=str(repository),wing='alpha'));items=symbols()
            refreshed=affected('A');assert cases(refreshed)==cases(after),refreshed;validate(refreshed)
            report=dict(status='passed',versions=mcp.call('index_status',{})['versions'],checks=['changed_class_invocations_only','availability_not_call_evidence','explicit_disable','separate_from_call_modes','captured_base_dependency','nested_trait_dependency','depth_budget','output_limit','shared_work_budget','direct_host_budget_independence','warm_equivalence','full_resolution_equivalence','alias_only_Git_before_after','Git_and_index_read_only','stale_host_exclusion','source_sync','project_isolation'],PHP_executed=False,PHPUnit_executed=False,production_database_touched=False,runtime_coverage_measured=False,safe_to_skip_unlisted_tests=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
