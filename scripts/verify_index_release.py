#!/usr/bin/env python3
"""Verify local release provenance against retained source and raw Cargo evidence."""
import json
from pathlib import Path
import sys
from build_index_release import compiler_artifact, digest, tree_identity, verify_snapshot


def verify(manifest_path, binary):
    manifest_path, binary = Path(manifest_path).resolve(), Path(binary).resolve()
    root = manifest_path.parent
    manifest = json.loads(manifest_path.read_text())
    state = json.loads((root / 'state.json').read_text())
    if manifest.get('contract_version') != 1 or manifest.get('status') != 'built_verified' or state.get('status') != 'built_verified':
        raise ValueError('release build is not terminal and verified')
    if manifest['profile'] != 'release' or digest(binary) != manifest['binary_sha256'] or state['binary_sha256'] != manifest['binary_sha256']:
        raise ValueError('release profile or binary identity mismatch')
    if Path(manifest['binary_path']).resolve() != binary:
        raise ValueError('requested executable is not the frozen release artifact')
    files = json.loads((root / 'source-inputs.json').read_text())
    if digest(root / 'source-inputs.json') != manifest['source_inputs_sha256'] or tree_identity(files) != manifest['source_tree_sha256']:
        raise ValueError('source inventory identity mismatch')
    source = Path(state['snapshot']).resolve()
    if not source.is_relative_to(root):
        raise ValueError('source snapshot escaped the owned build root')
    verify_snapshot(source, files)
    if digest(source / 'Cargo.lock') != manifest['cargo_lock_sha256']:
        raise ValueError('locked dependency identity mismatch')
    for name, field in [('cargo.jsonl', 'cargo_json_sha256'), ('cargo-stderr.log', 'cargo_stderr_sha256')]:
        if digest(root / name) != manifest[field]:
            raise ValueError('raw Cargo evidence changed')
    artifact = compiler_artifact(root / 'cargo.jsonl')
    if artifact != manifest['compiler_artifact']:
        raise ValueError('compiler artifact differs from raw Cargo events')
    executable = Path(artifact['executable']).resolve()
    if not executable.is_relative_to(root / 'target') or digest(executable) != manifest['binary_sha256']:
        raise ValueError('compiler output and frozen executable differ')
    if Path(artifact['manifest_path']).resolve() != source / 'Cargo.toml':
        raise ValueError('compiler package manifest is not the frozen source')
    if Path(artifact['target']['src_path']).resolve() != source / 'src/main.rs':
        raise ValueError('compiler entrypoint is not the frozen server source')
    command = manifest['command']
    if command != state['command'] or not {'--release', '--locked', '--offline'}.issubset(command):
        raise ValueError('release/locked/offline compiler invocation missing')
    settings = {command[i + 1] for i, part in enumerate(command[:-1]) if part == '--config'}
    required = {'build.rustflags=[]', 'build.rustc-wrapper=""', 'build.rustc-workspace-wrapper=""',
        'profile.release.opt-level=3', 'profile.release.debug-assertions=false', 'profile.release.lto="thin"',
        'profile.release.codegen-units=1', 'profile.release.strip="debuginfo"'}
    if not required.issubset(settings):
        raise ValueError('explicit release compiler controls missing')
    driver_hash = digest(source / 'scripts/build_index_release.py')
    if driver_hash != manifest['driver_sha256'] or driver_hash != state['build_driver_sha256']:
        raise ValueError('executed build driver and captured driver differ')
    for field in ['source_tree_sha256', 'source_inputs_sha256', 'cargo_lock_sha256', 'tools', 'rustc_verbose_version', 'cargo_version']:
        if manifest[field] != state[field]:
            raise ValueError('build state/provenance mismatch: ' + field)
    if not manifest['source_unchanged_during_build'] or not manifest['compiler_executables_unchanged'] or state['cargo_exit_code'] != 0:
        raise ValueError('build input/compiler integrity was not established')
    return dict(contract_version=1, status='verified_release_artifact', profile=artifact['profile'],
        binary_sha256=manifest['binary_sha256'], manifest_sha256=digest(manifest_path),
        source_tree_sha256=manifest['source_tree_sha256'], source_files=len(files),
        source_snapshot_currently_intact=True, raw_cargo_evidence_intact=True,
        compiler_output_matches_frozen_binary=True, cargo_controls_verified=True,
        runtime_qualified=False, performance_qualified=False,
        limitations=manifest['limitations'])


if __name__ == '__main__':
    result = verify(Path(sys.argv[1]), Path(sys.argv[2]))
    with Path(sys.argv[3]).open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
