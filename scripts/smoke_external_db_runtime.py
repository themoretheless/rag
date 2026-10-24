"""Real stdio + HTTP MCP assertions, using only URLs of owned fixtures."""
import json, os, queue, socket, subprocess, tempfile, threading, time
from pathlib import Path
import urllib.request

def exercise(fixture_env, stop_mysql, mutate_schema=None):
    binary=Path('target/debug/rag-mcp').resolve()
    with tempfile.TemporaryDirectory(prefix='rag-external-mcp-') as directory:
        root=Path(directory); sources=root/'sources';sources.mkdir()
        (sources/'001.sql').write_text('CREATE TABLE child(a INT, b INT, missing TEXT, "a""b" INT); SELECT * FROM child; INSERT INTO absent_table(a) VALUES(1);')
        env={k:v for k,v in os.environ.items() if not k.startswith('RAG_')}
        for k,v in fixture_env.items():
            if k in ('RAG_TEST_EXTERNAL_PG_URL','RAG_TEST_EXTERNAL_MY_URL'):env[k]=v
        targets={}
        for alias,dialect,var,schema in [('pg','postgresql','RAG_TEST_EXTERNAL_PG_URL','public'),('my','mysql','RAG_TEST_EXTERNAL_MY_URL','app')]:
            targets[alias]=dict(dialect=dialect,url_env=var,project='fixture',schemas=[schema],tls='disable',samples_enabled=True,sample_columns={schema:{'child':['a','b','email'],'events':['domain_user_id','state','states','permissions']}})
        with socket.socket() as listener:
            listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
        read,write,admin='r'*48,'w'*48,'a'*48
        env.update(RAG_DB_PATH=str(root/'test.duckdb'),RAG_INGEST_ROOTS=str(sources),RAG_EMBEDDING_PROVIDER='mock',RAG_EMBEDDING_DIMS='16',RAG_LLM_ENABLED='false',RAG_TOOLS='full',RAG_HTTP_BIND=f'127.0.0.1:{port}',RAG_HTTP_READ_TOKEN=read,RAG_HTTP_WRITE_TOKEN=write,RAG_HTTP_ADMIN_TOKEN=admin,RAG_EXTERNAL_DATABASES=json.dumps(targets))
        with (root/'stderr').open('w') as stderr:
            process=subprocess.Popen([str(binary)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=stderr,text=True,env=env)
            incoming=queue.Queue()
            def receive():
                for line in process.stdout:incoming.put(json.loads(line))
                incoming.put(None)
            threading.Thread(target=receive,daemon=True).start();sequence=0
            def request(method,params):
                nonlocal sequence
                sequence+=1
                process.stdin.write(json.dumps(dict(jsonrpc='2.0',id=sequence,method=method,params=params))+'\n');process.stdin.flush()
                deadline=time.monotonic()+45
                while True:
                    response=incoming.get(timeout=max(.01,deadline-time.monotonic()))
                    assert response is not None,'MCP process exited'
                    if response.get('id')==sequence:return response
            def unpack(response):
                assert 'error' not in response,response
                result=response['result'];assert not result.get('isError'),result
                return json.loads(next(c['text'] for c in result['content'] if c['type']=='text'))
            def call(name,**arguments):return unpack(request('tools/call',dict(name=name,arguments=arguments)))
            def http(name,arguments,token):
                req=urllib.request.Request(f'http://127.0.0.1:{port}/mcp',data=json.dumps(dict(jsonrpc='2.0',id=900,method='tools/call',params=dict(name=name,arguments=arguments))).encode(),headers={'Content-Type':'application/json','Accept':'application/json, text/event-stream','Authorization':f'Bearer {token}'})
                with urllib.request.urlopen(req,timeout=35) as response:
                    if 'application/json' in response.headers.get('Content-Type',''):return json.load(response)
                    for line in response:
                        if line.startswith(b'data:'):
                            result=json.loads(line[5:].strip())
                            if result.get('id')==900:return result
                raise AssertionError('missing HTTP MCP response')
            def failed(response):return 'error' in response or response.get('result',{}).get('isError',False)
            try:
                initialized=request('initialize',dict(protocolVersion='2024-11-05',capabilities={},clientInfo=dict(name='external-db-fixture',version='1')))
                assert 'error' not in initialized
                process.stdin.write(json.dumps(dict(jsonrpc='2.0',method='notifications/initialized'))+'\n');process.stdin.flush()
                names={t['name'] for t in request('tools/list',{})['result']['tools']}
                assert {'db_refresh','db_schema','db_table','db_relations','db_sample','db_drift','db_cross'}<=names
                assert not call('sync_sources',path=str(sources),wing='fixture').get('errors')
                assert call('schema',project='fixture',table='child')['total']==1
                assert call('table_usage',project='fixture',table='child',mode='read')['total']==1
                assert call('table_usage',project='fixture',table='absent_table',mode='write')['total']==1
                assert call('write_check',project='fixture',table='child',columns=['a'],values=1)['status']=='valid'
                assert call('write_check',project='fixture',table='child',columns=['a'],values=2)['status']=='invalid'
                for alias in ('pg','my'):
                    first=call('db_refresh',connection=alias)
                    assert first['tables']==(10 if alias=='pg' else 9),first
                    assert call('db_drift',connection=alias)['compared'] is False
                    table=call('db_table',connection=alias,table='child')['items'][0]
                    def check(table_name, **kwargs):
                        return call('write_check',connection=alias,table=table_name,**kwargs)
                    def codes(result): return {i['code'] for i in result['issues']}
                    assert check('users',values={'id':1})['status']=='valid'
                    assert 'MISSING_REQUIRED_COLUMN' in codes(check('users',values={}))
                    assert 'NULL_NOT_ALLOWED' in codes(check('users',values={'id':None}))
                    combined=check('child',columns=['computed'],values=2)
                    assert {'GENERATED_COLUMN_ASSIGNMENT','VALUES_COUNT_MISMATCH'}<=codes(combined),combined
                    assert 'GENERATED_COLUMN_ASSIGNMENT' not in codes(check('child',default_columns=['computed']))
                    assert 'GENERATED_COLUMN_ASSIGNMENT' not in codes(check('child',columns=['created_at']))
                    negative=check('child',values={'a':-1})
                    assert ('CHECK_CONSTRAINT_VIOLATION' in codes(negative)) == (alias=='pg'),negative
                    assert check('events',operation='update',values={'state':'in,progress'})['status']=='valid'
                    bad_enum=check('events',operation='update',values={'state':'UNDECLARED_PRIVATE_MARKER'})
                    assert bad_enum['status']==('invalid' if alias=='pg' else 'indeterminate'),bad_enum
                    assert 'UNDECLARED_PRIVATE_MARKER' not in json.dumps(bad_enum)
                    if alias=='pg':
                        assert 'CHECK_CONSTRAINT_VIOLATION' in codes(check('events',operation='update',values={'domain_user_id':0}))
                        assert check('events',operation='update',values={'domain_user_id':9})['status']=='valid'
                        assert check('events',operation='update',values={'states':['new']})['status']=='indeterminate'
                    assert table['name']=='child'
                    assert table['metadata']['complete']
                    assert table['column_traits']['individually_unique_columns']==['a"b']
                    ix=next(i for i in table['metadata']['indexes'] if i['name']=='child_pair')
                    assert [k['column'] for k in ix['keys']]==['b','a'] and ix['keys'][0]['direction']=='DESC'
                    assert any(c['kind']=='CHECK' for c in table['metadata']['constraints'])
                    view=call('db_table',connection=alias,table='child_view')['items'][0]['metadata']['view']
                    assert view['definition_hash'] and view['complete']
                    deps=call('db_relations',connection=alias,table='nested_view')['items']
                    assert any(d['kind']=='view_dependency' and d['mapping']['target_table']=='child_view' for d in deps)
                    assert call('db_relations',connection=alias,table='child')['total']==2
                    relations=call('db_relations',connection=alias,table='events')
                    assert relations['inference_available'] and relations['total']==1 and relations['total_inferred']==5,relations
                    assert all(r['provenance']=='declared' for r in relations['items'])
                    assert all(r['provenance']=='inferred' and r['confidence']<=0.4 for r in relations['inferred'])
                    mappings={r['mapping']['column']:r['mapping']['target_table'] for r in relations['inferred']}
                    assert mappings=={'user_id':'users','UserID':'users','AuthorUserID':'users','company_id':'companies','domain_user_id':'users'},mappings
                    assert call('db_relations',connection=alias,table='events',relation_kind='inferred')['declared']==[]
                    assert call('db_relations',connection=alias,table='events',relation_kind='declared')['inferred']==[]
                    assert call('db_relations',connection=alias,table='events',limit=1)['inferred_truncated']
                    historic_inferred=relations['inferred']
                    event=call('db_table',connection=alias,table='events')['items'][0]
                    assert event['inferred_relations']==historic_inferred
                    assert call('db_relations',connection=alias,table='events',snapshot_id=first['snapshot_id'])['inferred']==historic_inferred
                    assert call('db_sample',connection=alias,table='child')['items'][0]['sample']['collected'] is False
                    cross=call('db_cross',connection=alias)
                    assert any(u['table']=='absent_table' and u['resolution']=='not_observed_in_visible_live_schema' for u in cross['uses'])
                    assert cross['migrations'][0]['migration_columns_not_observed_live']==['missing']
                    call('db_refresh',connection=alias,include_samples=True)
                    assert call('db_drift',connection=alias)['total']==0
                    assert call('db_relations',connection=alias,table='events')['inferred']==historic_inferred
                    sample=call('db_sample',connection=alias,table='child')['items'][0]['sample']
                    assert sample['collected'] and sample['columns']==['a','b']
                    assert sample['rows'][0]['a'] is None
                    typed_sample=call('db_sample',connection=alias,table='events')['items'][0]['sample']
                    assert typed_sample['collected'] and typed_sample['rows'][0]['state']['value']=='in,progress'
                    if alias=='pg': assert typed_sample['rows'][0]['domain_user_id']['value']=='9'
                    else: assert typed_sample['rows'][0]['permissions']['value']=='read,write' 
                    assert call('db_sample',connection=alias,table='child',snapshot_id=first['snapshot_id'])['items'][0]['sample']['collected'] is False
                    assert not unpack(http('db_schema',dict(connection=alias),read))['live_query']
                    assert unpack(http('write_check',dict(connection=alias,table='users',values={'id':1}),read))['status']=='valid'
                    for token in (read,write): assert failed(http('db_refresh',dict(connection=alias),token))
                    assert unpack(http('db_refresh',dict(connection=alias),admin))['readonly']
                    if mutate_schema:
                        baseline=call('db_schema',connection=alias)['snapshot_id']
                        mutate_schema(alias)
                        call('db_refresh',connection=alias)
                        drift=call('db_drift',connection=alias,base_snapshot_id=baseline)
                        assert drift['compared'] and drift['complete_within_visible_scope'],drift
                        assert {c['table'] for c in drift['changes']}=={'child','child_view'},drift
                        table_change=next(c for c in drift['changes'] if c['table']=='child')
                        assert any(i['name']=='child_new_idx' for i in table_change['after']['metadata']['indexes'])
                        assert table_change['before']['columns']==table_change['after']['columns']
                        view_change=next(c for c in drift['changes'] if c['table']=='child_view')
                        assert view_change['before']['metadata']['view']['definition_hash']!=view_change['after']['metadata']['view']['definition_hash']
                        if alias=='pg':
                            baseline=call('db_schema',connection=alias)['snapshot_id']
                            mutate_schema(alias,identity_only=True)
                            call('db_refresh',connection=alias)
                            identity=call('db_drift',connection=alias,base_snapshot_id=baseline)
                            assert identity['total']==1 and identity['changes'][0]['table']=='child',identity
                            change=identity['changes'][0]
                            assert next(c for c in change['before']['columns'] if c['name']=='id')['identity_kind']=='BY DEFAULT'
                            assert next(c for c in change['after']['columns'] if c['name']=='id')['identity_kind']=='ALWAYS'
                            assert 'IDENTITY_ALWAYS_EXPLICIT_ASSIGNMENT' in codes(check('child',values={'id':12}))
                            assert 'IDENTITY_ALWAYS_EXPLICIT_ASSIGNMENT' not in codes(check('child',values={'id':12},overriding_system_value=True))
                            assert 'IDENTITY_ALWAYS_EXPLICIT_ASSIGNMENT' not in codes(check('child',values={'id':12},snapshot_id=baseline))
                    if mutate_schema:
                        for stage in (['enum','domain'] if alias=='pg' else ['enum','set']):
                            baseline=call('db_schema',connection=alias)['snapshot_id']
                            mutate_schema(alias,types_only=stage)
                            call('db_refresh',connection=alias)
                            changed=call('db_drift',connection=alias,base_snapshot_id=baseline)
                            assert changed['total']==1 and changed['changes'][0]['table']=='events',changed
                            after=changed['changes'][0]['after']
                            definitions=after['metadata']['types']['definitions']
                            if alias=='pg':
                                assert next(d for d in definitions if d['name']=='order_status')['enum_members'][-1]=='archived'
                                if stage=='enum': assert changed['changes'][0]['before']['columns']==after['columns']
                                else:
                                    assert '11' in next(c for c in after['columns'] if c['name']=='domain_user_id')['effective_default_sql']
                                    assert next(d for d in definitions if d['name']=='user_identifier')['constraints'][0]['validated'] is False
                            elif stage=='enum':assert next(d for d in definitions if d['name']=='state')['members'][-1]=='archived'
                            else:assert next(d for d in definitions if d['name']=='permissions')['members'][-1]=='admin'
                        historical=call('db_table',connection=alias,table='events',snapshot_id=first['snapshot_id'])['items'][0]['metadata']['types']['definitions']
                        assert not any('archived' in d.get('members',d.get('enum_members',[])) for d in historical)
                assert failed(request('tools/call',dict(name='db_refresh',arguments=dict(connection='pg',url='SELECT 1'))))
                assert call('db_schema',connection='pg',table="child';DROP TABLE parent;--")['total']==0
                before=call('db_schema',connection='my')['snapshot_id']
                stop_mysql()
                assert failed(request('tools/call',dict(name='db_refresh',arguments=dict(connection='my'))))
                assert call('db_schema',connection='my')['snapshot_id']==before
                offline=call('write_check',connection='my',table='users',values={'id':1})
                assert offline['status']=='valid' and offline['executed'] is False and offline['live_query'] is False
                return dict(http_read_write_check=True,migration_schema_usage_and_write_check=True,captured_write_check=True,write_check_after_remote_shutdown=True,write_check_generated_identity_enum_domain=True,stdio=True,http_read=True,http_admin_refresh=True,read_and_write_cannot_refresh=True,offline_reads_after_remote_failure=True,historical_samples=True,indexes_checks_views=True,enum_domain_set_metadata=True,enum_domain_set_drift=True,known_type_samples=True,frozen_inferred_relations=True,declared_and_inferred_separate=True,actual_schema_drift=mutate_schema is not None,identity_mode_drift=mutate_schema is not None,aliases=['pg','my'],tools=sorted(n for n in names if n.startswith('db_')))
            finally:
                process.terminate()
                try:process.wait(timeout=20)
                except subprocess.TimeoutExpired:process.kill();process.wait(timeout=10)
                errors=(root/'stderr').read_text()
                for key in ('RAG_TEST_EXTERNAL_PG_URL','RAG_TEST_EXTERNAL_MY_URL'):
                    assert fixture_env[key] not in errors,'URL leaked into server stderr'
