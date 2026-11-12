#!/usr/bin/env python3
"""Real MCP qualification for conditional lexical trait frames in trace_path."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-trace-php-contexts-') as temp:
        root=Path(temp).resolve();source=root/'sources'
        fixtures={
            'alpha/trait.php':'<?php trait T {public function run(){self::leaf();}}',
            'alpha/hosts.php':'<?php class A {use T;public static function leaf(){} public static function other(){} public function entry(){$this->run();}} class B {use T;public static function leaf(){} public static function other(){} public function entry(){$this->run();}}',
            'alpha/shared.php':'<?php trait Shared {public function shared(){self::leaf();}} class Root {public static function leaf(){}} class S1 extends Root {use Shared;} class S2 extends Root {use Shared;}',
            'alpha/nested.php':'<?php trait Inner {public function inner(){self::leaf();}} trait Outer {public function outer(){$this->inner();}} class P1 {use Inner;public static function leaf(){}} class P2 {use Inner;public static function leaf(){}} class N1 extends P1 {use Outer;public function entry(){$this->outer();}} class N2 extends P2 {use Outer;public function entry(){$this->outer();}}',
            'alpha/values.php':'<?php trait ValueTrait {public function value(){self::leaf();}} class V1 {use ValueTrait;public static function leaf(){} public function entry(){$f=$this->value(...);}} class V2 {use ValueTrait;public static function leaf(){} public function entry(){$f=$this->value(...);}}',
            'alpha/enum.php':'<?php trait EnumTrait {public static function work(){self::leaf();}} enum E {case One;use EnumTrait;public static function leaf(){}} function begin(){E::work();}',
            'beta/foreign.php':'<?php trait T {public function run(){self::leaf();}} class Foreign {use T;public static function leaf(){}}',
        }
        for name,body in fixtures.items():
            p=source/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                sync=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not sync.get('errors'),sync
            def symbols():
                result=mcp.call('symbol',dict(project='alpha',limit=500));assert not result.get('parse_errors'),result
                return {s['qualified_name']:s for s in result['items']}
            items=symbols()
            def trace(start,end,**kwargs):return mcp.call('trace_path',dict(project='alpha',**{'from':items[start]['id'],'to':items[end]['id']},**kwargs))
            def validate(result):
                assert not result['runtime_path_proven'] and not result['absence_proven'],result
                for path in result['paths']:
                    assert path['captured_trait_frames_matched'] and not path['runtime_path_proven'],path
                    assert len(path['symbols'])==len(path['node_contexts'])==path['length']+1,path
                    owners={s['id'] for s in path['trait_context_symbols']}
                    for i,step in enumerate(path['steps']):
                        src,dst=(path['symbols'][i+1],path['symbols'][i]) if step['reversed'] else (path['symbols'][i],path['symbols'][i+1])
                        assert step['source']['id']==src['id'] and step['target']['id']==dst['id'],step
                        contexts=(path['node_contexts'][i+1],path['node_contexts'][i]) if step['reversed'] else (path['node_contexts'][i],path['node_contexts'][i+1])
                        assert (step['source_context'],step['target_context'])==contexts,step
                        for ctx in contexts:
                            if ctx is not None:assert ctx in owners,path
                        ref=step['evidence']['reference'];assert ref['document_id']==src['document_id'] and ref['content_hash']==src['content_hash'],step
                        e=step['evidence'].get('context_evidence')
                        if e:
                            assert any(dst['id'] in row['candidates'] and (e['kind']=='php_captured_trait_entry' or row['owner']==step['source_context']) and row.get('target_owner')==step['target_context'] for row in e['contexts']),step
            good=trace('A::entry','A::leaf');assert good['status']=='found',good;validate(good)
            assert good['paths'][0]['steps'][1]['aggregate_binding_ambiguous'] and not good['paths'][0]['steps'][1]['ambiguous_target'],good
            for direction in ('forward','both'):
                no=trace('A::entry','B::leaf',direction=direction,include_ambiguous=True);assert no['status']=='not_found_in_bounded_graph' and not no['paths'],no
            back=trace('A::leaf','A::entry',direction='back');assert back['status']=='found',back;validate(back)
            assert trace('B::leaf','A::entry',direction='back')['status']=='not_found_in_bounded_graph'
            shared=trace('Shared::shared','Root::leaf');assert len(shared['paths'])==2,shared;validate(shared)
            assert shared['paths'][0]['node_contexts'][0]!=shared['paths'][1]['node_contexts'][0],shared
            nested=trace('N1::entry','P1::leaf');assert nested['status']=='found' and nested['paths'][0]['length']==3,nested;validate(nested)
            assert trace('N1::entry','P2::leaf')['status']=='not_found_in_bounded_graph'
            assert trace('V1::entry','V1::leaf')['status']=='not_found_in_bounded_graph'
            values=trace('V1::entry','V1::leaf',relations='references');assert values['status']=='found',values;validate(values)
            assert values['paths'][0]['steps'][0]['relation']=='bound_reference',values
            assert trace('V1::entry','V2::leaf',relations='all')['status']=='not_found_in_bounded_graph'
            enum=trace('begin','E::leaf');assert enum['status']=='found',enum;validate(enum)
            assert any(s['kind']=='enum_declaration' for s in enum['paths'][0]['trait_context_symbols']),enum
            shallow=trace('A::entry','A::leaf',max_depth=1);assert shallow['depth_limit_reached'],shallow
            capped=trace('Shared::shared','Root::leaf',max_paths=1);assert capped['result_limit_reached'] and len(capped['paths'])==1,capped
            for budget in (1,5,10,20,40):
                bounded=trace('A::entry','A::leaf',max_work=budget);assert bounded['work']<=budget,bounded
            assert good==trace('A::entry','A::leaf')
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            host=source/'alpha/hosts.php';host.write_text(fixtures['alpha/hosts.php']+'\n')
            mcp.call('ingest_file',dict(path=str(host),wing='alpha'))
            stale=trace('A::entry','A::leaf');assert stale['stale_documents'] and not stale['paths'],stale
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            trait=source/'alpha/trait.php';trait.write_text(fixtures['alpha/trait.php'].replace('self::leaf','self::other'))
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            assert trace('A::entry','A::leaf')['status']=='not_found_in_bounded_graph'
            refreshed=trace('A::entry','A::other');assert refreshed['status']=='found',refreshed;validate(refreshed)
            assert trace('A::entry','B::other')['status']=='not_found_in_bounded_graph'
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            result=dict(passed=True,versions=mcp.call('index_status',{})['versions'],checks=['forward-back-both-context-matching','incompatible-consumers-excluded-even-with-ambiguity-opt-in','within-context-target-uniqueness','conditional-root-alternatives','nested-inherited-trait-frames','value-reference-not-call','enum-consumer','step-context-and-source-provenance','project-scope','depth-result-work-budgets','determinism','full-cache-equivalence','stale-owner-exclusion','source-sync-target-refresh'],runtime_path_proven=False,production_database_touched=False,source_applications_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))


if __name__=='__main__':main()
