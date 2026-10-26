#!/usr/bin/env python3
"""Real MCP source pagination: UTF-8 bytes, view conflicts, CSV IDs and overlaps."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-code-cursors-') as temporary:
        root=Path(temporary).resolve();source=root/'sources';alpha=source/'alpha';alpha.mkdir(parents=True)
        path=alpha/'large.py';body="def target(): return '"+'α'*120005+"'\n";path.write_text(body)
        nested=alpha/'nested.py';nested_body="class C:\n    def first(self):\n        return 1\n    def second(self):\n        return 2\n\ndef tail(): return 3\n";nested.write_text(nested_body)
        with StartupClient(binary,root,source) as mcp:
            sync=mcp.call('sync_sources',dict(path=str(alpha),wing='alpha'));assert not sync.get('errors'),sync
            def read(**kw):return mcp.call('code',dict(project='alpha',**kw))
            def error(contains,**kw):
                try:read(**kw)
                except AssertionError as exc:
                    payload=exc.args[0];assert contains in json.dumps(payload),payload
                    return payload
                raise AssertionError('expected '+contains)
            q=dict(symbol='target',max_chars=11)
            first=read(**q);assert not first['read_complete'] and first['next_cursor'],first
            held_cursor=first['next_cursor']
            warm=mcp.call('sync_sources',dict(path=str(alpha),wing='alpha'))
            assert not warm.get('errors') and not warm['code_index']['revision_created'],warm
            beta=source/'beta';beta.mkdir();foreign=beta/'foreign.py';foreign.write_text('def foreign(): pass\n')
            mcp.call('ingest_file',dict(path=str(foreign),wing='beta'))
            q.update(cursor=held_cursor,max_chars=100000)
            result_text=first['code'];pages=1
            while True:
                page=read(**q);pages+=1;assert pages<=4
                result_text+=page['code']
                if page['read_complete']:assert page['next_cursor'] is None;break
                assert page['next_cursor']!=q['cursor'];q['cursor']=page['next_cursor']
            assert result_text==body.rstrip('\n'),(len(result_text),len(body))
            a=body.encode().index('α'.encode())
            byte=read(symbol='target',start_byte=a,end_byte=a+4);assert byte['code']=='αα',byte
            error('UTF-8',symbol='target',start_byte=a+1)
            error('CODE_VIEW_CHANGED',symbol='target',cursor=held_cursor,start_byte=a)
            error('invalid code cursor',symbol='target',cursor='bad')
            symbols=mcp.call('symbol',dict(path=str(nested),project='alpha'))['items']
            ids=','.join(s['id'] for s in symbols)
            csv=read(entry_ids=ids);array=read(entry_ids=ids.split(','));assert csv==array,(csv,array)
            q=dict(path=str(nested),include_containers=True,limit=1,max_chars=7)
            seen=set();pages_nested=0
            while True:
                page=read(**q);pages_nested+=1;assert pages_nested<50
                for item in page['items']:
                    for part in item['segments']:
                        start,end=part['start_byte'],part['end_byte']
                        assert nested_body.encode()[start:end].decode()==part['code'],part
                        assert not seen.intersection(range(start,end)),part
                        seen.update(range(start,end))
                if page['next_cursor'] is None:break
                q['cursor']=page['next_cursor']
            expected=set().union(*(set(range(s['start_byte'],s['end_byte'])) for s in symbols));assert seen==expected,(len(seen),len(expected))
            target=mcp.call('symbol',dict(path=str(path),project='alpha',symbol='target'))['items'][0]
            doc=mcp.call('get_document',dict(document_id=target['document_id'],include_chunks=True,chunk_limit=2))
            chunks=doc['chunks'];assert len(chunks)==2,chunks
            chunk_ranges=[(c['char_start'],c['char_end']) for c in chunks]
            assert chunk_ranges[0][0]<chunk_ranges[1][0]<chunk_ranges[0][1]<chunk_ranges[1][1],chunk_ranges
            q=dict(entry_ids=[c['id'] for c in chunks],limit=1,max_chars=1000)
            text='';seen=set();pages_chunks=0
            while True:
                page=read(**q);pages_chunks+=1;assert pages_chunks<30
                for item in page['items']:
                    for part in item['segments']:
                        start,end=part['start_byte'],part['end_byte'];assert not seen.intersection(range(start,end)),part
                        seen.update(range(start,end));text+=part['code']
                if page['next_cursor'] is None:break
                q['cursor']=page['next_cursor']
            start=min(c['char_start'] for c in chunks);end=max(c['char_end'] for c in chunks)
            assert text==body[start:end],(len(text),start,end)
            path.write_text(body.replace('α','β',1));mcp.call('ingest_file',dict(path=str(path),wing='alpha'))
            conflict=error('CODE_VIEW_CHANGED',symbol='target',cursor=held_cursor)
            assert conflict['error']['data']['code']=='CONFLICT',conflict
            mcp.call('sync_sources',dict(path=str(alpha),wing='alpha'))
            error('CODE_VIEW_CHANGED',symbol='target',cursor=held_cursor)
            report=dict(passed=True,checks=['large_single_line_complete_pagination','changed_page_budget','cursor_progress','no_op_sync_preserves_cursor','foreign_ingest_without_publication_preserves_scoped_cursor','UTF8_byte_window','split_UTF8_rejection','query_conflict','malformed_cursor','CSV_array_ID_equivalence','nested_container_record_and_text_pagination','no_repeated_bytes_across_pages','actual_chunk_overlap_pagination','stale_view_conflict','republished_view_conflict'],large_pages=pages,nested_pages=pages_nested,chunk_pages=pages_chunks,actual_chunk_character_ranges=chunk_ranges,actual_chunks_partially_overlap=True,versions=mcp.call('index_status',{})['versions'],production_database_touched=False,source_applications_executed=False,retained_historical_snapshot=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':main()
