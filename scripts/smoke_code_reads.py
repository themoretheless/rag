#!/usr/bin/env python3
"""Real MCP stored-source path/window/ID reads, no app execution or production DB."""
import ast
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-code-reads-') as temporary:
        root=Path(temporary).resolve();source=root/'sources';source.mkdir();path=source/'a.py'
        body="class C:\n    def first(self):\n        return 'αβγ'\n    def second(self):\n        return 2\n\ndef tail():\n    return 'quasar_old'\n"
        path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            sync=mcp.call('sync_sources',dict(path=str(source),wing='alpha'));assert not sync.get('errors'),sync
            def read(**kw):return mcp.call('code',dict(project='alpha',**kw))
            symbols=mcp.call('symbol',dict(path=str(path),project='alpha'))['items'];tail=next(s for s in symbols if s['name']=='tail')
            legacy=read(symbol=tail['id']);assert "quasar_old" in legacy['code'] and legacy['symbol']==tail,legacy
            assert 'items' not in legacy and not legacy['filesystem_read'] and legacy['observation_consistency']=='single_project_scoped_read_transaction',legacy
            ranged=read(symbol=tail['id'],start=8,end=8);assert ranged['code']=="    return 'quasar_old'",ranged
            outside=read(symbol=tail['id'],start=999);assert outside['outside_window'] and outside['code']=='',outside
            selected=read(path=str(path));assert selected['nested_dropped']==2 and len(selected['items'])==2,selected
            reference=dict(available=False)
            reference_path=Path('/tmp/ragraf-compare-20261023/src/ragraf/facade.py')
            if reference_path.exists():
                function=next(n for n in ast.parse(reference_path.read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='_drop_nested')
                namespace={}
                exec(compile(ast.Module(body=[function],type_ignores=[]),str(reference_path),'exec'),namespace)
                expected=namespace['_drop_nested']([(s['path'],s['start_line'],s['end_line'],s['qualified_name']) for s in symbols])
                assert {i['symbol']['qualified_name'] for i in selected['items']}=={s[3] for s in expected},(selected,expected)
                reference=dict(available=True,pure_reference_function_only=True,reference_commit='075caf86dc2f6274b9c48487e521915bcdfeaf2c',multiline_nested_selection_equal=True)
            partitioned=read(path=str(path),include_containers=True)
            assert partitioned['spans_total']==4 and partitioned['nested_dropped']==0,partitioned
            seen=set()
            for item in partitioned['items']:
                for segment in item['segments']:
                    a,b=segment['start_byte'],segment['end_byte']
                    assert body.encode()[a:b].decode()==segment['code'],segment
                    assert not seen.intersection(range(a,b)),partitioned
                    seen.update(range(a,b))
            bounded=read(path=str(path),include_containers=True,max_chars=13)
            assert bounded['text_truncated'] and sum(len(s['code']) for i in bounded['items'] for s in i['segments'])==13,bounded
            limited=read(path=str(path),include_containers=True,limit=1)
            assert limited['spans_truncated'] and limited['retained_spans_total']==4,limited
            ids=read(entry_ids=[tail['id'],'missing'])
            assert ids['unresolved_entry_ids']==['missing'] and ids['items'][0]['symbol']['id']==tail['id'],ids
            doc=mcp.call('get_document',dict(document_id=tail['document_id'],include_chunks=True))
            assert doc['chunks'],doc
            chunk=doc['chunks'][0]
            by_chunk=read(entry_ids=[chunk['id']])
            assert by_chunk['found'] and not by_chunk['unresolved_entry_ids'],(by_chunk,chunk)
            returned=''.join(s['code'] for i in by_chunk['items'] for s in i['segments'])
            assert returned==chunk['content'],(by_chunk,chunk)
            foreign=mcp.call('code',dict(project='beta',entry_ids=[tail['id'],chunk['id']]))
            assert not foreign['found'] and len(foreign['unresolved_entry_ids'])==2,foreign
            updated=body.replace('quasar_old','quasar_new')
            path.write_text(updated)
            assert 'quasar_old' in read(symbol='tail')['code'],'unexpected filesystem fallback'
            mcp.call('ingest_file',dict(path=str(path),wing='alpha'))
            stale=read(path=str(path));assert not stale['found'] and stale['stale_documents'],stale
            mcp.call('sync_sources',dict(path=str(source),wing='alpha'))
            fresh=read(symbol='tail');assert 'quasar_new' in fresh['code'] and fresh['symbol']['content_hash']!=tail['content_hash'],fresh
            report=dict(passed=True,reference_comparison=reference,checks=['legacy_single_symbol','inclusive_window','outside_window','path_nested_dedup','container_unique_segments','UTF8_byte_proof','shared_character_budget','independent_span_limit','symbol_IDs_and_missing','current_chunk_ID_to_source','project_scope','no_live_disk_fallback','stale_exclusion','sync_refresh','single_transaction_envelope'],versions=mcp.call('index_status',{})['versions'],production_database_touched=False,source_applications_executed=False,whole_reference_or_scale_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
