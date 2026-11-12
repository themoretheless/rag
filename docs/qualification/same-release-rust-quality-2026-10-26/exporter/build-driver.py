#!/usr/bin/env python3
"""Build the owned-store exporter against the server's existing release libraries."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from build_index_release import digest
from verify_index_release import verify


def libraries(root, events, name):
    found = []
    for event in events:
        if event.get('reason') != 'compiler-artifact' or event.get('target', {}).get('name') != name or 'lib' not in event['target']['kind']:
            continue
        profile = event['profile']
        if profile['opt_level'] != '3' or profile['debug_assertions'] or profile['test']:
            continue
        paths = [Path(p).resolve() for p in event['filenames'] if p.endswith('.rlib')]
        if len(paths) == 1 and paths[0].is_relative_to(root / 'target'):
            found.append((event, paths[0]))
    if len(found) != 1:
        raise ValueError('missing unique optimized runtime library: ' + name)
    return found[0]


def main():
    root, output = [Path(p).resolve() for p in sys.argv[1:3]]
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    server_manifest = root / 'release-manifest.json'
    server = json.loads(server_manifest.read_text())
    original_verification = verify(server_manifest, Path(server['binary_path']))
    state = json.loads((root / 'state.json').read_text())
    source = Path(state['snapshot']).resolve()
    events = [json.loads(line) for line in (root / 'cargo.jsonl').read_text().splitlines()]
    rag, rag_path = libraries(root, events, 'rag_mcp')
    serde, serde_path = libraries(root, events, 'serde_json')
    if rag['features'] != server['compiler_artifact']['features'] or rag['manifest_path'] != str(source / 'Cargo.toml'):
        raise ValueError('exporter library differs from server package/features')
    dependency_paths = set()
    native_paths = set()
    linked_files = set()
    for event in events:
        if event.get('reason') == 'compiler-artifact':
            for name in event['filenames']:
                path = Path(name).resolve()
                if path.suffix in ('.rlib', '.rmeta', '.dylib', '.so'):
                    if not path.is_relative_to(root / 'target'):
                        raise ValueError('dependency escaped owned release target')
                    dependency_paths.add(path.parent)
                    linked_files.add(path)
        if event.get('reason') == 'build-script-executed':
            native_paths.update(event.get('linked_paths', []))
    inputs = {str(p): digest(p) for p in sorted(linked_files)}
    # Current rustc uses metadata stubs in rlibs; provide the recorded full rmeta
    # and keep the matching rlib/search paths for the final link.
    rag_metadata = rag_path.with_suffix('.rmeta')
    serde_metadata = serde_path.with_suffix('.rmeta')
    if str(rag_metadata) not in rag['filenames'] or str(serde_metadata) not in serde['filenames']:
        raise ValueError('matching full runtime metadata is unavailable')

    rustc = Path(server['tools']['rustc']['path'])
    if digest(rustc) != server['tools']['rustc']['sha256']:
        raise ValueError('server compiler changed before exporter build')
    helper_source = source / 'examples/code_quality_capture.rs'
    executable = output / 'code_quality_capture'
    command = [str(rustc), str(helper_source), '--crate-name', 'code_quality_capture', '--crate-type', 'bin', '--edition=2021',
        '--extern', 'rag_mcp=' + str(rag_metadata), '--extern', 'rag_mcp=' + str(rag_path),
        '--extern', 'serde_json=' + str(serde_metadata), '--extern', 'serde_json=' + str(serde_path),
        '-C', 'opt-level=3', '-C', 'debug-assertions=off', '-C', 'overflow-checks=off', '-C', 'debuginfo=0',
        '-C', 'lto=thin', '-C', 'codegen-units=1', '-C', 'strip=debuginfo', '-o', str(executable)]
    for path in sorted(dependency_paths):
        command += ['-L', 'dependency=' + str(path)]
    for path in sorted(native_paths):
        command += ['-L', path]
    report = dict(contract_version=1, status='building', started_unix=time.time(),
        driver_sha256=digest(__file__), server_verification=original_verification,
        server_manifest_sha256=digest(server_manifest), server_binary_sha256=server['binary_sha256'],
        source_tree_sha256=server['source_tree_sha256'], helper_source_sha256=digest(helper_source),
        rag_library=dict(path=str(rag_path), sha256=inputs[str(rag_path)], metadata_path=str(rag_metadata), metadata_sha256=inputs[str(rag_metadata)], cargo_artifact=rag),
        serde_library=dict(path=str(serde_path), sha256=inputs[str(serde_path)], metadata_path=str(serde_metadata), metadata_sha256=inputs[str(serde_metadata)], cargo_artifact=serde),
        compiled_library_inputs=inputs, command=command, rustc=server['tools']['rustc'],
        production_database_opened=False, runtime_quality_qualified=False, performance_qualified=False,
        limitations=['same compiled server libraries, not identical final executable instructions after thin LTO',
            'toolchain/SDK external inputs remain subject to the server build provenance limits'])
    def save():
        pending = output / 'state.pending.json'
        pending.write_text(json.dumps(report, indent=2) + '\n')
        os.replace(pending, output / 'state.json')
    save()
    env = {k: v for k, v in os.environ.items() if not k.startswith(('RAG_', 'CARGO_PROFILE_', 'CARGO_BUILD_')) and k not in
           {'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTUP_TOOLCHAIN'}}
    try:
        with (output / 'rustc-stdout.log').open('x') as stdout, (output / 'rustc-stderr.log').open('x') as stderr:
            process = subprocess.Popen(command, cwd=source, env=env, stdout=stdout, stderr=stderr)
            report['build_pid'] = process.pid
            save()
            code = process.wait()
        report['rustc_exit_code'] = code
        if code:
            raise RuntimeError('exporter rustc failed; stdout/stderr retained')
        if any(digest(p) != h for p, h in inputs.items()) or digest(helper_source) != report['helper_source_sha256']:
            raise ValueError('compiled library/source inputs changed during exporter build')
        if digest(rustc) != report['rustc']['sha256'] or digest(__file__) != report['driver_sha256']:
            raise ValueError('compiler or driver changed during exporter build')
        if verify(server_manifest, Path(server['binary_path'])) != original_verification:
            raise ValueError('server release provenance changed during exporter build')
        report.update(status='built_verified', helper_binary_sha256=digest(executable), completed_unix=time.time(),
                      library_inputs_unchanged=True, server_binary_unchanged=True)
        save()
        with (output / 'helper-manifest.json').open('x') as destination:
            destination.write(json.dumps(report, indent=2) + '\n')
        print(json.dumps(dict(status=report['status'], binary=str(executable), sha256=report['helper_binary_sha256'])))
    except Exception as error:
        report.update(status='failed', error=dict(type=type(error).__name__, message=str(error)), completed_unix=time.time())
        save()
        raise


if __name__ == '__main__':
    main()
