#!/usr/bin/env python3
"""Real MCP qualification for captured trait-frame impact correlation."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-trait-paths-') as temp:
        root=Path(temp).resolve();source=root/'sources'
        fixtures={
            'alpha/trait.php': '<?php trait T {public function run(){self::save();}}',
            'alpha/hosts.php': "<?php class A {use T;public static function save(){$q='INSERT INTO users(id) VALUES(1)';} public function entry(){$this->run();} public function other(){}} class B {use T;public static function save(){$q='INSERT INTO other(id) VALUES(1)';} public function entry(){$this->run();}}",
            'alpha/shared.php': '<?php trait Shared {public function shared(){self::leaf();}} class Common {public static function leaf(){}} class S1 extends Common {use Shared;public function entry(){$this->shared();}} class S2 extends Common {use Shared;public function entry(){$this->shared();}}',
            'alpha/nested.php': '<?php trait Inner {public function inner(){self::leaf();}} trait Outer {public function outer(){$this->inner();}} class Base1 {use Inner;public static function leaf(){}} class Base2 {use Inner;public static function leaf(){}} class N1 extends Base1 {use Outer;public function entry(){$this->outer();}} class N2 extends Base2 {use Outer;public function entry(){$this->outer();}}',
            'alpha/values.php': '<?php trait ValueTrait {public function value(){self::leaf();}} class V1 {use ValueTrait;public static function leaf(){} public function entry(){$f=$this->value(...);}} class V2 {use ValueTrait;public static function leaf(){} public function entry(){$f=$this->value(...);}}',
            'beta/foreign.php': '<?php trait T {public function run(){self::save();}} class Foreign {use T;public static function save(){} public function entry(){$this->run();}}',
        }
        for name,body in fixtures.items():
            p=source/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project));assert not result.get('errors'),result
            def symbols():
                result=mcp.call('symbol',dict(project='alpha',limit=500));assert not result.get('parse_errors'),result
                return {s['qualified_name']:s for s in result['items']}
            items=symbols()
            def names(result):
                ids={s['id']:n for n,s in items.items()}
                return {ids[r['symbol_id']] for r in result['items']}
            def impact(name,**kw):return mcp.call('impact',dict(symbol=items[name]['id'],impact_project='alpha',depth=10,limit=500,**kw))
            a=impact('A::save');assert names(a)=={'T::run','A::entry'},a
            assert a['trait_context_traversal']['transitions_pruned']>0 and not a['trait_context_traversal']['metadata_incomplete'],a
            b=impact('B::save');assert names(b)=={'T::run','B::entry'},b
            shared=impact('Common::leaf');assert names(shared)=={'Shared::shared','S1::entry','S2::entry'},shared
            nested=impact('Base1::leaf');assert names(nested)=={'Inner::inner','Outer::outer','N1::entry'},nested
            assert names(impact('V1::leaf'))=={'ValueTrait::value'}
            for mode in ('references','all'):
                value=impact('V1::leaf',impact_relations=mode);assert names(value)=={'ValueTrait::value','V1::entry'},value
            short=mcp.call('impact',dict(symbol=items['A::save']['id'],impact_project='alpha',depth=1))
            assert short['depth_limited'] and names(short)=={'T::run'},short
            capped=mcp.call('impact',dict(symbol=items['Common::leaf']['id'],impact_project='alpha',depth=10,limit=1));assert capped['truncated'] and len(capped['items'])==1,capped
            entry=mcp.call('callees',dict(symbol=items['A::entry']['id']))['items'][0]
            assert entry['context_evidence']['kind']=='php_captured_trait_entry',entry
            row=entry['context_evidence']['contexts'][0];assert row['owner']==row['target_owner']==items['A']['id'],row
            table=mcp.call('impact',dict(impact_table='users',impact_project='alpha',impact_table_mode='write',depth=10))
            assert table['direct_owner_symbols']==1 and names(table)=={'T::run','A::entry'},table
            reads=mcp.call('impact',dict(impact_table='users',impact_project='alpha',impact_table_mode='read',depth=10));assert reads['direct_total']==0 and not reads['items'],reads
            warm=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));assert warm['code_index']['indexed']==0,warm
            assert impact('A::save')==a
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            before=mcp.call('index_status',dict(project='alpha'))['index_rev']
            hosts=source/'alpha/hosts.php';hosts.write_text(fixtures['alpha/hosts.php'].replace('public function entry(){$this->run();} public function other()', 'public function entry(){$this->other();} public function other()'))
            mcp.call('ingest_file',dict(path=str(hosts),wing='alpha'))
            stale=mcp.call('callees',dict(symbol=items['T::run']['id']));assert stale['stale_documents'] and not any(e['candidates'] for e in stale['items']),stale
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'));items=symbols()
            assert names(impact('A::save'))=={'T::run'}
            assert names(impact('B::save'))=={'T::run','B::entry'}
            after=mcp.call('index_status',dict(project='alpha'))['index_rev']
            delta=mcp.call('diff',dict(before=before,after=after,project='alpha'));assert delta['php_trait_path_comparison']['available'],delta
            assert any(r['kind']=='php_trait_entry_context_candidate' for r in delta['relations_removed']),delta
            foreign=mcp.call('diff',dict(before=before,after=after,project='beta'));assert not foreign['relations_removed'] and not foreign['edges_added'],foreign
            full=mcp.call('index_code',dict(force_full_resolution=True));assert full['resolution']['equivalent_to_existing'] is True,full
            result=dict(passed=True,versions=mcp.call('index_status',{})['versions'],checks=['incompatible-consumers-pruned','all-compatible-context-states-retained','nested-inherited-trait-frames','callable-values-not-calls','reference-and-all-modes','depth-and-output-caps','entry-frame-evidence','SQL-table-impact-correlation','SQL-write-read-filter-isolation','warm-reuse','full-cache-equivalence','stale-host-exclusion','entry-change-refresh','frozen-entry-diff','project-isolation'],runtime_context_correlation_proven=False,source_applications_executed=False,production_database_touched=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))


if __name__=='__main__':main()
