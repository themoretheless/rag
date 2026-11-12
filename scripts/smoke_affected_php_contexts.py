#!/usr/bin/env python3
"""Isolated MCP PHPUnit recommendations with coherent captured trait frames."""
import json
from pathlib import Path
import sys
import subprocess
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-affected-php-contexts-') as tmp:
        root=Path(tmp).resolve();source=root/'sources'
        fixtures={
            'alpha/trait.php':'<?php trait T {public function run(){self::leaf();}}',
            'alpha/hosts.php':r'<?php use PHPUnit\Framework\TestCase as Base; use PHPUnit\Framework\Attributes\Test as Check; class A extends Base {use T;public static function leaf(){} public function testA(){$this->run();}} class B extends Base {use T;public static function leaf(){} #[Check] public function verifiesB(){$this->run();}}',
            'alpha/shared.php':r'<?php class Root {public static function leaf(){}} trait Shared {public function work(){self::leaf();}} class S1 extends \PHPUnit\Framework\TestCase {use Shared;public static function leaf(){Root::leaf();} public function testS1(){$this->work();}} class S2 extends \PHPUnit\Framework\TestCase {use Shared;public static function leaf(){Root::leaf();} public function testS2(){$this->work();}}',
            'alpha/values.php':r'<?php trait Values {public function work(){self::leaf();}} class V1 extends \PHPUnit\Framework\TestCase {use Values;public static function leaf(){} public function testV1(){$f=$this->work(...);}} class V2 extends \PHPUnit\Framework\TestCase {use Values;public static function leaf(){} public function testV2(){$f=$this->work(...);}}',
            'beta/foreign.php':r'<?php trait T {public function run(){self::leaf();}} class A extends \PHPUnit\Framework\TestCase {use T;public static function leaf(){} public function testForeign(){$this->run();}}',
        }
        for name,body in fixtures.items():
            p=source/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():
                return {s['qualified_name']:s for s in mcp.call('symbol',dict(project='alpha',limit=500))['items']}
            items=symbols()
            def affected(name,**kw):
                return mcp.call('affected_tests',dict(symbols=[items[name]['id']],project='alpha',**kw))
            def names(result):return {r['symbol']['qualified_name'] for r in result['items']}
            def validate(result):
                for flag in ('coverage_complete','test_discovery_complete','runtime_coverage_measured','safe_to_skip_unlisted_tests','absence_proven'):
                    assert result[flag] is False,result
                for item in result['items']:
                    path=item['path'];assert path['complete_to_seed'] and path['captured_trait_frames_matched'],item
                    assert not path['runtime_context_correlation_proven'] and not item['commands'],item
                    nodes=path['symbols'];steps=path['steps'];assert len(nodes)==len(steps)+1,item
                    context_ids={s['id'] for s in path['trait_context_symbols']}
                    for i,step in enumerate(steps):
                        assert step['source_symbol_id']==nodes[i]['id'] and step['target_symbol_id']==nodes[i+1]['id'],step
                        if step['relation']=='phpunit_host_candidate':
                            evidence=step['evidence'];assert not evidence['call_edge'] and evidence['body_content_hash']==nodes[i]['content_hash'] and evidence['body_document_id']==nodes[i]['document_id'] and evidence['host_symbol_id']==nodes[i+1]['id'] and evidence['host_content_hash']==nodes[i+1]['content_hash'],step
                        elif step['relation']=='inheritance_candidate':
                            provenance=step['evidence']['source_provenance'];assert provenance['content_hash']==nodes[i]['content_hash'] and provenance['document_id']==nodes[i]['document_id'],step
                        else:
                            ref=step['evidence']['reference'];assert ref['content_hash']==nodes[i]['content_hash'] and ref['document_id']==nodes[i]['document_id'],step
                        if i:assert steps[i-1]['target_context']==step['source_context'],item
                        for field in ('source_context','target_context'):
                            if step[field] is not None:assert step[field] in context_ids,item
                        evidence=step['evidence'].get('context_evidence')
                        if evidence:
                            assert any(nodes[i+1]['id'] in row['candidates'] and (evidence['kind']=='php_captured_trait_entry' or row['owner']==step['source_context']) and row.get('target_owner')==step['target_context'] for row in evidence['contexts']),step
                    assert item['seed_symbol_id']==nodes[-1]['id'],item
            a=affected('A::leaf');assert names(a)=={'A::testA'},a;validate(a)
            b=affected('B::leaf');assert names(b)=={'B::verifiesB'},b;validate(b)
            assert a['trait_context_traversal']['transitions_pruned']>0,a
            assert names(affected('T::run'))=={'A::testA','B::verifiesB'}
            shared=affected('Root::leaf');assert names(shared)=={'S1::testS1','S2::testS2'},shared;validate(shared)
            capped=affected('Root::leaf',limit=1);assert len(capped['items'])==1 and capped['related_tests_total_in_examined_graph']==2 and capped['truncated'],capped
            value=affected('V1::leaf');assert names(value)=={'V1::testV1'},value;validate(value)
            assert not affected('V1::leaf',relations='calls')['items']
            all_relations=affected('A::leaf',relations='all');assert names(all_relations)=={'A::testA'},all_relations;validate(all_relations)
            direct=affected('A::testA');assert direct['items'][0]['reason']=='changed_test' and not direct['items'][0]['path']['steps'],direct;validate(direct)
            shallow=affected('A::leaf',depth=1);assert not shallow['items'] and shallow['depth_limited'],shallow
            for budget in (1,10,30,50,100):
                bounded=affected('A::leaf',max_work=budget);assert bounded['work']<=budget,bounded
            warm=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert warm['code_index']['indexed']==0,warm
            assert affected('A::leaf')==a
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            assert affected('A::leaf')['items']==a['items']
            hosts=source/'alpha/hosts.php';hosts.write_text(fixtures['alpha/hosts.php'].replace('$this->run();',''))
            mcp.call('ingest_file',dict(path=str(hosts),wing='alpha'))
            stale_trait=affected('T::run');assert not stale_trait['items'] and stale_trait['stale_documents'],stale_trait
            stale=affected('Root::leaf');assert names(stale)=={'S1::testS1','S2::testS2'} and stale['stale_documents'],stale
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            assert not affected('A::leaf')['items'] and not affected('B::leaf')['items']
            repository=source/'git-case';repository.mkdir()
            def git(*args):
                return subprocess.run(['git','-C',str(repository),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
            (repository/'trait.php').write_text(fixtures['alpha/trait.php'])
            git_hosts=fixtures['alpha/hosts.php'].replace('function leaf(){}','function leaf(){return 1;}')
            (repository/'hosts.php').write_text(git_hosts)
            git('init','-q');git('add','.')
            git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm','base')
            (repository/'hosts.php').write_text(git_hosts.replace('return 1','return 2',1))
            changed=mcp.call('detect_changes',dict(repository=str(repository),project='git-case',scope='all',include_tests=True,depth=10))
            assert not changed['persistent_index_used'],changed
            for side in ('before_impact','after_impact'):
                recommendations=changed[side]['affected_tests']
                assert names(recommendations)=={'A::testA'},changed
                validate(recommendations)
            report=dict(status='passed',versions=mcp.call('index_status',{})['versions'],checks=['compatible_consumer_witnesses','incompatible_consumers_pruned','attribute_tests','shared_trait_multiple_states','test_output_limit_after_traversal','value_references_vs_calls','all_relations','direct_changed_test','depth_work_bounds','warm_equivalence','full_resolution_equivalence','stale_exclusion','source_sync','project_isolation','exact_source_witnesses','git_before_after_PHP_recommendations'],PHP_executed=False,PHPUnit_executed=False,production_database_touched=False,runtime_coverage_measured=False,safe_to_skip_unlisted_tests=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
