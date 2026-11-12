#!/usr/bin/env python3
"""Qualify captured PHP class chains through real MCP, without running PHP."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-inheritance-') as temporary:
        root = Path(temporary).resolve()
        source = root/'sources'
        fixtures = {
            'alpha/base.php': '<?php namespace Lib; class Base {public static function save(){} protected function work(){} private function hidden(){} public function replace(){}} class Mid extends Base {} class Other {public static function save(){} protected function work(){} public function replace(){}}',
            'alpha/app.php': '<?php namespace App; use Lib\\Mid as ParentClass; class Child extends ParentClass {public function replace(){} public function run(){self::save();$this->work();parent::replace();$this->replace();$this->hidden();} public static function invalid(){parent::work();static::save();}} function main(){Child::save();}',
            'alpha/traits.php': '<?php namespace App; trait T {public static function save(){}} class Composed extends \\Lib\\Base {use T;} function unknown(){Composed::save();}',
            'beta/base.php': '<?php namespace Lib; class Base {public static function save(){}} class Mid extends Base {}',
        }
        for name, body in fixtures.items():
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
            def calls(name):
                return mcp.call('callees',dict(symbol=items[name]['id']))
            def target_map(result):
                return {e['reference']['expression']:e['candidates'] for e in result['items']}
            run=target_map(calls('App\\Child::run'))
            assert run['self::save']==[items['Lib\\Base::save']['id']],run
            assert run['$this->work']==[items['Lib\\Base::work']['id']],run
            assert run['parent::replace']==[items['Lib\\Base::replace']['id']],run
            assert run['$this->replace']==[items['App\\Child::replace']['id']],run
            assert not run['$this->hidden'],run
            assert all(not e['candidates'] for e in calls('App\\Child::invalid')['items'])
            assert target_map(calls('App\\unknown'))['Composed::save']==[items['App\\T::save']['id']]
            inheritance=mcp.call('inheritance',dict(symbol=items['App\\Child']['id']))
            assert inheritance['items'][0]['candidates']==[items['Lib\\Mid']['id']],inheritance
            overrides=inheritance['method_relations']['items']
            assert len(overrides)==1 and overrides[0]['candidates']==[items['Lib\\Base::replace']['id']],inheritance
            assert not overrides[0]['semantic_override_proven'],inheritance
            callers=mcp.call('callers',dict(symbol=items['Lib\\Base::save']['id']))
            assert {e['reference']['caller'] for e in callers['items']}=={items['App\\Child::run']['id'],items['App\\main']['id']},callers
            impact=mcp.call('impact',dict(symbol=items['Lib\\Base::save']['id'],impact_project='alpha'))
            assert {r['symbol_id'] for r in impact['items']}=={items['App\\Child::run']['id'],items['App\\main']['id']},impact
            # Candidate outputs retain original source byte evidence.
            for e in calls('App\\Child::run')['items']:
                r=e['reference']
                assert fixtures['alpha/app.php'].encode()[r['start_byte']:r['start_byte']+len(r['expression'].encode())].decode()==r['expression'],r
            warm=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            assert warm['code_index']['indexed']==0,warm
            before=mcp.call('index_status',dict(project='alpha'))['index_rev']
            app=source/'alpha/app.php'
            app.write_text(fixtures['alpha/app.php'].replace('Lib\\Mid as ParentClass','Lib\\Other as ParentClass'))
            mcp.call('ingest_file',dict(path=str(app),wing='alpha'))
            assert not mcp.call('symbol',dict(path=str(app)))['items']
            stale=mcp.call('callers',dict(symbol=items['Lib\\Base::save']['id']))
            assert not stale['items'],stale
            sync=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            updated=target_map(calls('App\\Child::run'))
            assert updated['self::save']==[items['Lib\\Other::save']['id']],updated
            assert updated['$this->work']==[items['Lib\\Other::work']['id']],updated
            after=mcp.call('index_status',dict(project='alpha'))['index_rev']
            delta=mcp.call('diff',dict(before=before,after=after,project='alpha'))
            assert delta['php_inheritance_comparison']['available'],delta
            assert not delta['php_inheritance_comparison']['source_semantic_change_proven'],delta
            assert delta['edges_added_total'] and delta['edges_removed_total'],delta
            assert any(r['kind']=='inheritance' for r in delta['relations_added']),delta
            assert any(r['kind']=='method_override_candidate' for r in delta['relations_added']),delta
            assert mcp.call('diff',dict(before=before,after=after,project='beta'))['relations_added']==[]
            # Change only the base file: callers must lose the removed inherited method.
            base_path=source/'alpha/base.php'
            base_path.write_text(fixtures['alpha/base.php'].replace('class Other {public static function save(){}', 'class Other {public static function different(){}'))
            mcp.call('ingest_file',dict(path=str(base_path),wing='alpha'))
            stale_calls=calls('App\\Child::run')
            assert stale_calls['stale_documents'] and not target_map(stale_calls)['self::save'],stale_calls
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            removed=calls('App\\Child::run')
            assert not target_map(removed)['self::save'],removed
            assert mcp.call('diff',dict(before=before,after=after,project='alpha'))==delta
            # Force a full resolution without publishing a new revision.
            full=mcp.call('index_code',dict(force_full_resolution=True))
            assert full['resolution']['equivalent_to_existing'] is True,full
            report=dict(passed=True,versions=mcp.call('index_status',{})['versions'],checks=['imported-multilevel-base','self-and-this-inherited-methods','parent-bypasses-local-override','private-and-static-context-guards','late-static-and-trait-precedence','inheritance-and-override-candidates','callers-and-impact','source-byte-provenance','project-isolation','warm-reuse','stale-exclusion','import-only-base-refresh','immutable-diff','base-file-only-method-removal','historical-diff-remains-frozen','forced-full-cache-equivalence'],compiler_runtime_verified=False,production_database_touched=False,source_applications_executed=False)
            if len(sys.argv)>2:
                Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':
    main()
