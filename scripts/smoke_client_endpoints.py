#!/usr/bin/env python3
"""Real MCP: jQuery/Axios candidates, provenance, references/impact/history/sync."""
import ast
import json
import re
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else 'target/debug/rag-mcp'
    with tempfile.TemporaryDirectory(prefix='rag-client-endpoints-') as temporary:
        root = Path(temporary).resolve()
        source = root / 'sources'
        source.mkdir()
        path = source / 'client.ts'
        body = "function load(){ $.get('/get'); axios.post('/save',{method:'DELETE'}); $.ajax({url:'/ajax',method:'PATCH'}); axios.get('/conflict',{method:'POST'}); } function caller(){load();} $.ajax({url:'/unknown'}); $.ajax({url:'/excluded',...opts});"
        false_calls = "const sample = \"$.get('/string')\"; // $.get('/comment')\n"
        body += '\n' + false_calls
        reference = dict(available=False)
        reference_path=Path('/tmp/ragraf-compare-20261023/src/ragraf/extract/php_ast.py')
        if reference_path.exists():
            assignment=next(n for n in ast.parse(reference_path.read_text()).body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='ENDPOINT_CALL_RE' for t in n.targets))
            namespace={'re':re}
            exec(compile(ast.Module(body=[assignment],type_ignores=[]),str(reference_path),'exec'),namespace)
            false_hits=namespace['ENDPOINT_CALL_RE'].findall(false_calls)
            assert set(false_hits)=={'/string','/comment'}, false_hits
            reference=dict(available=True,pure_reference_regex_only=True,reference_commit='075caf86dc2f6274b9c48487e521915bcdfeaf2c',reference_false_hits_on_controlled_comment_and_string_fixture=false_hits,local_false_hits=0)
        path.write_text(body)
        html = source / 'view.html'
        html_body = "<p>α</p>\n<script>\n$.post('/inline'); axios({url:'/object',method:'DELETE'});\n</script><script type='application/json'>$.get('/inert')</script>"
        html.write_text(html_body)
        with StartupClient(binary, root, source) as mcp:
            sync = mcp.call('sync_sources', dict(path=str(source), wing='clients'))
            assert not sync.get('errors'), sync
            result = mcp.call('routes', dict(project='clients', kind='endpoint'))
            before = result['index_rev']
            rows = {r['route']: r for r in result['items']}
            assert set(rows) == {'/get','/save','/ajax','/conflict','/unknown','/inline','/object'}, result
            assert rows['/get']['request_method'] == 'GET' and rows['/get']['request_method_basis'] == 'client_method_name_candidate'
            assert rows['/save']['request_method'] == 'POST' and rows['/save']['reference_kind'] == 'js:axios:post'
            assert rows['/ajax']['request_method'] == 'PATCH' and rows['/ajax']['request_method_basis'] == 'explicit_options'
            assert rows['/conflict'].get('request_method') is None and rows['/unknown'].get('request_method') is None
            assert rows['/inline']['line'] == 3
            assert html_body.encode()[rows['/inline']['start_byte']:].startswith(b"$.post('/inline')")
            assert not result['detail']['client_library_identity_verified'], result
            assert all(r['kind']=='http_endpoint_reference' and r['symbol_id'] is None for r in rows.values())
            post = mcp.call('routes',dict(project='clients',kind='endpoint',request_method='post'))
            assert {r['route'] for r in post['items']} == {'/save','/inline'}, post
            selected = mcp.call('routes',dict(project='clients',reference_kind='js:axios:post'))
            assert selected['total']==1, selected
            refs = mcp.call('references',dict(route='/save',project='clients'))
            assert refs['total']==1 and refs['items'][0]['request_method_basis']=='client_method_name_candidate', refs
            impact = mcp.call('impact',dict(impact_route='/save',impact_project='clients'))
            assert impact['direct_total']==1 and impact['direct_occurrences'][0]['request_method_basis']=='client_method_name_candidate', impact
            symbols=mcp.call('symbol',dict(path=str(path)))['items']
            names={s['id']:s['name'] for s in symbols}
            assert {names[r['symbol_id']] for r in impact['items']}=={'caller'}, impact
            warm=mcp.call('sync_sources',dict(path=str(source),wing='clients'))
            assert warm['code_index']['indexed']==0, warm
            path.write_text(body.replace("method:'PATCH'", "method:'PUT'"))
            mcp.call('ingest_file',dict(path=str(path),wing='clients'))
            stale=mcp.call('routes',dict(project='clients',kind='endpoint'))
            assert all(r['path']!=str(path) for r in stale['items']) and stale['stale_documents'], stale
            mcp.call('sync_sources',dict(path=str(source),wing='clients'))
            fresh=mcp.call('routes',dict(project='clients',kind='endpoint',route='/ajax'))
            assert fresh['items'][0]['request_method']=='PUT', fresh
            diff=mcp.call('diff',dict(before=before,after=fresh['index_rev'],project='clients'))
            assert diff['client_endpoint_relations_available'], diff
            added=[r for r in diff['relations_added'] if r.get('reference_kind')=='js:jquery:ajax']
            removed=[r for r in diff['relations_removed'] if r.get('reference_kind')=='js:jquery:ajax']
            assert len(added)==1 and added[0]['request_method']=='PUT', diff
            assert len(removed)==1 and removed[0]['request_method']=='PATCH', diff
            foreign=mcp.call('routes',dict(project='other',kind='endpoint'))
            assert foreign['items']==[], foreign
            report=dict(passed=True,reference_comparison=reference,checks=['jquery_and_axios_syntax','method_basis','conflict_and_dynamic_guards','inline_utf8_source_offsets','method_and_kind_filters','references_basis','literal_impact_owner_and_callers','warm_cache','stale_exclusion','sync_refresh','frozen_method_diff','project_isolation'],versions=mcp.call('index_status',{})['versions'],client_library_or_runtime_request_proven=False,production_database_touched=False,source_applications_executed=False)
            if len(sys.argv)>2:
                Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report))


if __name__=='__main__':
    main()
