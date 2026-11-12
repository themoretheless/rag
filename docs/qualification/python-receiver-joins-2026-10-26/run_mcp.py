"""Run from repository root; copy one binary and record isolated MCP checks."""
import concurrent.futures
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

q = Path(__file__).resolve().parent
paths = ['Cargo.toml', 'Cargo.lock', 'src/code_index.rs', 'src/code_index/data_flow.rs',
         'src/code_index/data_flow/capture.rs', 'src/code_index/data_flow/capture/completion.rs', 'src/code_index/data_flow/finally_tests.rs', 'src/code_index/data_flow/except_tests.rs', 'src/code_index/resolve/python_names/tests.rs', 'src/code_index/resolve/python_names/flow.rs', 'src/code_index/resolve.rs', 'src/code_index/dead_code.rs', 'src/code_index/affected_tests.rs', 'src/code_index/data_flow/query.rs',
         'src/code_index/data_flow/tests.rs', 'src/code_index/data_flow/diff.rs', 'src/code_index/data_flow/diff/tests.rs', 'src/code_index/resolve/python_names.rs',
         'src/code_index/history.rs', 'src/code_index/status.rs', 'src/mcp/analysis.rs',
         'scripts/smoke_python_data_flow.py', 'scripts/smoke_python_value_diff.py', 'scripts/smoke_python_finally.py', 'scripts/smoke_python_except.py', 'scripts/smoke_python_assignment_order.py', 'scripts/smoke_python_receiver_joins.py']
def hashes():
    return {p: hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in paths}
before = hashes()
with tempfile.TemporaryDirectory(prefix='rag-branch-qualified-') as tmp:
    binary = Path(tmp)/'rag-mcp'
    shutil.copy2('target/debug/rag-mcp', binary)
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    names = ['python_receiver_joins', 'python_assignment_order', 'python_except', 'python_finally', 'python_value_diff', 'python_data_flow', 'python_bindings', 'table_impact', 'phpunit_host_impact',
             'phpunit_effective', 'affected_tests', 'trace_php_contexts', 'git_php_contexts']
    def run(name):
        with (q/(name+'.log')).open('w') as log:
            result = subprocess.run(['python3', 'scripts/smoke_'+name+'.py', str(binary),
                                     str(q/(name+'.json'))], stdout=log, stderr=subprocess.STDOUT)
        return name, result.returncode
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        codes = dict(pool.map(run, names))
    after = hashes()
    report = dict(status='passed' if all(c == 0 for c in codes.values()) else 'failed',
                  mcp_exit_codes=codes, binary_sha256=digest,
                  binary_unchanged=digest == hashlib.sha256(binary.read_bytes()).hexdigest(),
                  source_hashes_before=before, source_hashes_after=after,
                  selected_sources_unchanged=before == after, shared_source_frozen=False,
                  production_database_touched=False)
    (q/'verification.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(dict(status=report['status'], mcp_exit_codes=codes, binary_sha256=digest)))
    raise SystemExit(0 if report['status'] == 'passed' else 1)
