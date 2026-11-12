#!/usr/bin/env python3
"""Source trait candidates over real MCP; isolated DB/mock embeddings, no PHP execution."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-php-traits-') as temp:
        root=Path(temp).resolve()
        source=root/'sources'
        fixtures={
            'alpha/traits.php': '<?php namespace Lib; function helper(){} trait A {public static function save(){helper();} protected function work(){} private function secret(){}} trait B {public static function save(){}} trait Nested {use A;}',
            'alpha/app.php': '<?php namespace App; use Lib\\Nested as N; use Lib\\B as TB; class Base {public static function save(){}} class Host extends Base {use N,TB {N::save insteadof TB; TB::save as fallback; save as chosen; N::save as private hidden;} public function run(){self::chosen();self::hidden();$this->work();$this->secret();parent::save();}} class Child extends Host {public function child(){$this->secret();}} function main(){Host::save();Host::fallback();Host::chosen();Host::hidden();$f=Host::chosen(...);}',
            'alpha/guards.php': '<?php namespace App; class Conflict {use \\Lib\\A,\\Lib\\B;} class Unknown {use Missing;} function guard(){Conflict::save();Unknown::save();} enum Status {case One;use \\Lib\\Nested;} function enumUse(){Status::save();}',
            'beta/traits.php': '<?php namespace Lib; trait A {public static function save(){}} trait B {public static function save(){}}',
        }
        for name,body in fixtures.items():
            path=source/name
            path.parent.mkdir(parents=True,exist_ok=True)
            path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ('alpha','beta'):
                result=mcp.call('sync_sources',dict(path=str(source/project),wing=project))
                assert not result.get('errors'),result
            def symbols():
                result=mcp.call('symbol',dict(project='alpha',limit=500))
                assert not result.get('parse_errors'),result
                return {s['qualified_name']:s for s in result['items']}
            items=symbols()
            def calls(name):return mcp.call('callees',dict(symbol=items[name]['id']))
            def targets(result):return {e['reference']['expression']:e['candidates'] for e in result['items']}
            main=targets(calls('App\\main'))
            assert main['Host::save']==[items['Lib\\A::save']['id']],main
            assert main['Host::fallback']==[items['Lib\\B::save']['id']],main
            assert main['Host::chosen']==[items['Lib\\A::save']['id']],main
            assert not main['Host::hidden'],main
            run=targets(calls('App\\Host::run'))
            assert run['self::chosen']==[items['Lib\\A::save']['id']],run
            assert run['self::hidden']==[items['Lib\\A::save']['id']],run
            assert run['$this->work']==[items['Lib\\A::work']['id']],run
            assert run['$this->secret']==[items['Lib\\A::secret']['id']],run
            assert run['parent::save']==[items['App\\Base::save']['id']],run
            assert not targets(calls('App\\Child::child'))['$this->secret']
            assert targets(calls('App\\enumUse'))['Status::save']==[items['Lib\\A::save']['id']]
            guards=calls('App\\guard')
            assert all(not e['candidates'] for e in guards['items']),guards
            assert {e['resolution_basis'] for e in guards['items']}=={'PHP_TRAIT_METHOD_CONFLICT','PHP_TRAIT_TARGET_NOT_CAPTURED'},guards
            references=mcp.call('references',dict(symbol=items['Lib\\A::save']['id'],project='alpha'))
            assert any(r['expression']=='Host::chosen' and not r['is_call'] for r in references['items']),references
            assert len([e for e in calls('App\\main')['items'] if e['reference']['expression']=='Host::chosen'])==1
            relations=mcp.call('inheritance',dict(symbol=items['App\\Host']['id']))
            uses=[r for r in relations['items'] if r['relation']=='php_trait_use_candidate']
            assert {c for r in uses for c in r['candidates']}=={items['Lib\\Nested']['id'],items['Lib\\B']['id']},relations
            impact=mcp.call('impact',dict(symbol=items['Lib\\A::save']['id'],impact_project='alpha'))
            assert {items['App\\main']['id'],items['App\\Host::run']['id'],items['App\\enumUse']['id']}<={r['symbol_id'] for r in impact['items']},impact
            structural=mcp.call('impact',dict(symbol=items['Lib\\A']['id'],impact_project='alpha',impact_relations='all'))
            assert {items['Lib\\Nested']['id'],items['App\\Host']['id']}<={r['symbol_id'] for r in structural['items']},structural
            trait=items['Lib\\A::save']
            code=mcp.call('code',dict(symbol=trait['id']))
            assert code['code']==fixtures['alpha/traits.php'].encode()[trait['start_byte']:trait['end_byte']].decode(),code
            warm=mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            assert warm['code_index']['indexed']==0,warm
            before=mcp.call('index_status',dict(project='alpha'))['index_rev']
            app=source/'alpha/app.php'
            app.write_text(fixtures['alpha/app.php'].replace('N::save insteadof TB','TB::save insteadof N'))
            mcp.call('ingest_file',dict(path=str(app),wing='alpha'))
            assert not mcp.call('symbol',dict(path=str(app)))['items']
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            changed=targets(calls('App\\main'))
            assert changed['Host::save']==[items['Lib\\B::save']['id']],changed
            assert changed['Host::chosen']==[items['Lib\\B::save']['id']],changed
            assert targets(calls('App\\Host::run'))['self::hidden']==[items['Lib\\A::save']['id']]
            after=mcp.call('index_status',dict(project='alpha'))['index_rev']
            delta=mcp.call('diff',dict(before=before,after=after,project='alpha'))
            assert delta['php_traits_comparison']['available'] and not delta['php_traits_comparison']['source_semantic_change_proven'],delta
            assert delta['edges_added_total'] and delta['edges_removed_total'],delta
            foreign=mcp.call('diff',dict(before=before,after=after,project='beta'))
            assert not foreign['edges_added'] and not foreign['edges_removed'],foreign
            path=source/'alpha/traits.php'
            path.write_text(fixtures['alpha/traits.php'].replace('trait B {public static function save(){}','trait B {public static function replaced(){}'))
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'))
            assert not targets(calls('App\\main'))['Host::save']
            mcp.call('sync_sources',dict(path=str(source/'alpha'),wing='alpha'))
            items=symbols()
            assert not targets(calls('App\\main'))['Host::save']
            assert mcp.call('diff',dict(before=before,after=after,project='alpha'))==delta
            full=mcp.call('index_code',dict(force_full_resolution=True))
            assert not full['revision_created'] and full['resolution']['equivalent_to_existing'] is True,full
            result=dict(passed=True,versions=mcp.call('index_status',{})['versions'],checks=['nested-namespace-traits','class-trait-base-precedence','insteadof-selection','excluded-method-alias','unqualified-selected-alias','private-alias-host-visibility','private-not-inherited','enum-method-candidate','explicit-conflict-and-unknown-guards','callable-value-not-call','trait-use-relations','callers-and-impact','original-source-byte-identity','project-isolation','warm-reuse','stale-exclusion','selection-only-change','immutable-diff','trait-file-only-method-removal-no-fallback','full-cache-equivalence'],compiler_runtime_verified=False,production_database_touched=False,source_applications_executed=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(result,indent=2)+'\n')
            print(json.dumps(result))


if __name__=='__main__':main()
