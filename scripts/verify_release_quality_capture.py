#!/usr/bin/env python3
"""Verify exporter/source/library identity without opening any DuckDB store."""
import json
from pathlib import Path
import sys
from build_index_release import digest
from build_release_quality_capture import libraries
from verify_index_release import verify as verify_server


def verify(helper_manifest, server_manifest):
    helper_manifest, server_manifest = Path(helper_manifest).resolve(), Path(server_manifest).resolve()
    helper = json.loads(helper_manifest.read_text())
    server = json.loads(server_manifest.read_text())
    root = server_manifest.parent
    source = Path(json.loads((root / 'state.json').read_text())['snapshot'])
    server_proof = verify_server(server_manifest, Path(server['binary_path']))
    if json.loads((helper_manifest.parent / 'state.json').read_text()) != helper:
        raise ValueError('exporter final state and manifest differ')
    if helper.get('status') != 'built_verified' or helper.get('rustc_exit_code') != 0:
        raise ValueError('exporter build is not verified')
    if helper['server_manifest_sha256'] != digest(server_manifest) or helper['server_binary_sha256'] != server['binary_sha256']:
        raise ValueError('exporter belongs to another server artifact')
    if helper['source_tree_sha256'] != server['source_tree_sha256'] or helper['server_verification'] != server_proof:
        raise ValueError('exporter source/provenance pair changed')
    if digest(helper_manifest.parent / 'build-driver.py') != helper['driver_sha256']:
        raise ValueError('retained exporter build driver differs')
    command = helper['command']
    binary = helper_manifest.parent / 'code_quality_capture'
    if Path(command[command.index('-o')+1]).resolve() != binary or digest(binary) != helper['helper_binary_sha256']:
        raise ValueError('exporter executable identity mismatch')
    expected_source = source / 'examples/code_quality_capture.rs'
    if Path(command[1]) != expected_source or digest(expected_source) != helper['helper_source_sha256']:
        raise ValueError('exporter source is not the frozen guarded exporter')
    if command[0] != server['tools']['rustc']['path'] or helper['rustc'] != server['tools']['rustc']:
        raise ValueError('exporter compiler differs from server compiler')
    if digest(command[0]) != helper['rustc']['sha256']:
        raise ValueError('compiler executable changed')
    flags = {command[i+1] for i, arg in enumerate(command[:-1]) if arg == '-C'}
    if not {'opt-level=3', 'debug-assertions=off', 'overflow-checks=off', 'debuginfo=0', 'lto=thin', 'codegen-units=1', 'strip=debuginfo'}.issubset(flags):
        raise ValueError('exporter release controls missing')
    externs = {command[i+1] for i, arg in enumerate(command[:-1]) if arg == '--extern'}
    events = [json.loads(line) for line in (root / 'cargo.jsonl').read_text().splitlines()]
    for name, field in [('rag_mcp','rag_library'), ('serde_json','serde_library')]:
        artifact, path = libraries(root, events, name)
        saved = helper[field]
        metadata = path.with_suffix('.rmeta')
        if saved['cargo_artifact'] != artifact or Path(saved['path']) != path:
            raise ValueError('exporter runtime library does not match original Cargo output')
        if digest(path) != saved['sha256'] or digest(metadata) != saved['metadata_sha256'] or Path(saved['metadata_path']) != metadata:
            raise ValueError('exporter library or full metadata changed')
        if not {name+'='+str(path), name+'='+str(metadata)}.issubset(externs):
            raise ValueError('exporter must explicitly link full metadata and matching runtime objects')
    for path, expected in helper['compiled_library_inputs'].items():
        if not Path(path).resolve().is_relative_to(root / 'target') or digest(path) != expected:
            raise ValueError('original compiled library inputs changed')
    if not helper['library_inputs_unchanged'] or not helper['server_binary_unchanged']:
        raise ValueError('build input integrity was not established')
    return dict(status='verified_release_quality_pair', server_binary_sha256=server['binary_sha256'],
        helper_binary_sha256=helper['helper_binary_sha256'], source_tree_sha256=helper['source_tree_sha256'],
        helper_manifest_sha256=digest(helper_manifest), compiled_library_inputs=len(helper['compiled_library_inputs']),
        same_original_cargo_runtime_libraries=True, full_metadata_and_objects_explicit=True,
        no_dependency_feature_rebuild=True, production_database_opened=False,
        corpus_quality_qualified=False, performance_qualified=False,
        limitations=helper['limitations'])


if __name__ == '__main__':
    result = verify(Path(sys.argv[1]), Path(sys.argv[2]))
    with Path(sys.argv[3]).open('x') as output:
        output.write(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result))
