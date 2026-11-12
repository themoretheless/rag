#!/usr/bin/env python3
"""Actual Git + MCP qualification of context-preserving before/after witnesses."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=Path(sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp').resolve()
    with tempfile.TemporaryDirectory(prefix='rag-git-php-contexts-') as temp:
        root=Path(temp).resolve();source=root/'sources';source.mkdir()
        def git(*args):
            return subprocess.run(['git','-C',str(source),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE).stdout.decode().strip()
        def commit(message):git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-qm',message)
        git('init','-q');
        trait='<?php trait T {public function run(){self::leaf();}}'
        hosts='<?php class Common {public static function leaf(){return 1;}} class A extends Common {use T;public function entry(){$this->run();}} class B extends Common {use T;public function entry(){$this->run();}}'
        (source/'trait.php').write_text(trait);path=source/'hosts.php';path.write_text(hosts)
        git('add','.');commit('base');before=git('rev-parse','HEAD')
        path.write_text(hosts.replace('return 1','return 2'));git('add','hosts.php')
        index_before=(source/'.git/index').read_bytes();path.write_text(hosts.replace('return 1','return 3'))
        def names(result,side):return {r['symbol']['qualified_name'] for r in result[side]['items']}
        def validate(result):
            assert result['analysis_contract']['version']==2,result
            assert not result['persistent_index_used'] and result['graph_origin']=='ephemeral_git_source_snapshots',result
            for side in ('before_impact','after_impact'):
                for item in result[side]['items']:
                    p=item['path'];steps=p['steps'];symbols=p['symbols']
                    assert p['complete_to_seed'] and p['captured_trait_frames_matched'],item
                    assert len(steps)==item['depth'] and len(symbols)==len(steps)+1,item
                    assert symbols[-1]['id']==item['seed_symbol_id'] and item['seed_change_key'],item
                    assert not p['runtime_context_correlation_proven'],p
                    context_ids={s['id'] for s in p['trait_context_symbols']}
                    for i,step in enumerate(steps):
                        assert step['source_symbol_id']==symbols[i]['id'] and step['target_symbol_id']==symbols[i+1]['id'],step
                        if i:assert steps[i-1]['target_context']==step['source_context'],p
                        for field in ('source_context','target_context'):
                            if step[field] is not None:assert step[field] in context_ids,p
                        ref=step['evidence']['reference'];assert ref['document_id']==symbols[i]['document_id'] and ref['content_hash']==symbols[i]['content_hash'],step
                        e=step['evidence'].get('context_evidence')
                        if e:
                            matched=[row for row in e['contexts'] if step['target_symbol_id'] in row['candidates'] and (e['kind']=='php_captured_trait_entry' or row['owner']==step['source_context']) and (step['target_context'] is None or row.get('target_owner')==step['target_context'])]
                            assert matched,step
                            assert all(row['owner'] in context_ids for row in matched),p
        with StartupClient(binary,root,source) as mcp:
            args=dict(repository=str(source),project='fixture',depth=10)
            stage=mcp.call('detect_changes',dict(args,scope='staged'));validate(stage)
            work=mcp.call('detect_changes',dict(args,scope='working_tree'));validate(work)
            for result in (stage,work):
                for side in ('before_impact','after_impact'):
                    assert names(result,side)=={'T::run','A::entry','B::entry'},result
                    for owner in ('A','B'):
                        item=next(r for r in result[side]['items'] if r['symbol']['qualified_name']==owner+'::entry')
                        assert any(c['qualified_name']==owner for c in item['path']['trait_context_symbols']),item
            assert work==mcp.call('detect_changes',dict(args,scope='working_tree'))
            assert (source/'.git/index').read_bytes()==index_before and path.read_text()==hosts.replace('return 1','return 3')
            commit('staged');after=git('rev-parse','HEAD')
            frozen=mcp.call('detect_changes',dict(args,scope='commits',before=before,after=after));validate(frozen)
            path.write_text('<?php class Unrelated {}')
            assert frozen==mcp.call('detect_changes',dict(args,scope='commits',before=before,after=after))
            # Same target union, unchanged trait body: only consumer assignment changes.
            swapped='<?php class P1 {public static function leaf(){}} class P2 {public static function leaf(){}} class A extends P1 {use T;public function entry(){$this->run();}} class B extends P2 {use T;public function entry(){$this->run();}}'
            path.write_text(swapped);git('add','hosts.php');commit('consumer baseline')
            path.write_text(swapped.replace('A extends P1','A extends P2').replace('B extends P2','B extends P1'))
            change=mcp.call('detect_changes',dict(args,scope='working_tree'));validate(change)
            binding=next(c for c in change['changes'] if any(s['qualified_name']=='T::run' for s in c['before']+c['after']))
            assert binding['kind']=='binding_changed',binding
            # Byte/ID movement alone is not a binding change in another file.
            path.write_text(swapped);git('add','hosts.php')
            path.write_text(swapped.replace('<?php','<?php /* shift captured IDs */'))
            padding=mcp.call('detect_changes',dict(args,scope='working_tree'))
            assert padding['files_changed_total']==1 and padding['changed_symbol_groups_total']==0,padding
            # A's target change cannot create a witness for B's incompatible caller.
            separate='<?php class A {use T;public static function leaf(){return 1;} public function entry(){$this->run();}} class B {use T;public static function leaf(){} public function entry(){$this->run();}}'
            path.write_text(separate);git('add','hosts.php');path.write_text(separate.replace('return 1','return 2'))
            isolated=mcp.call('detect_changes',dict(args,scope='working_tree'));validate(isolated)
            for side in ('before_impact','after_impact'):assert names(isolated,side)=={'T::run','A::entry'},isolated
            shallow=mcp.call('detect_changes',dict(args,scope='working_tree',depth=1));validate(shallow)
            assert shallow['before_impact']['depth_limited'],shallow
            limited=mcp.call('detect_changes',dict(args,scope='working_tree',max_affected=1));validate(limited)
            assert limited['after_impact']['truncated'] and len(limited['after_impact']['items'])==1,limited
            # No ingestion is required and no source index revision is published.
            status=mcp.call('index_status',{})
            assert status['index_rev'] is None,status
            result=dict(passed=True,index_format=stage['index_format'],checks=['staged-and-working-context-paths','shared-body-per-class-witnesses','step-context-and-source-provenance','read-only-index-and-source','determinism','pinned-commit-independence','context-swap-binding-change','byte-ID-movement-not-binding-change','incompatible-caller-exclusion','depth-output-caps','no-persistent-index-needed'],runtime_context_correlation_proven=False,production_repository_touched=False,production_database_touched=False,source_applications_executed=False,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))


if __name__=='__main__':main()
