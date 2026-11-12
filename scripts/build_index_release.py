#!/usr/bin/env python3
"""Freeze working sources and retain Cargo release artifact/compiler provenance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def inventory(repository):
    names = set(subprocess.check_output(['git', '-C', str(repository), 'ls-files', '-z', '--cached', '--others', '--exclude-standard']).decode().split('\0')) - {''}
    result = {}
    for name in sorted(names):
        path = repository / name
        if Path(name).is_absolute() or '..' in Path(name).parts or path.is_symlink():
            raise ValueError('source inventory must use regular repository-local files')
        if path.exists():
            if not path.is_file():
                raise ValueError('unsupported source input')
            result[name] = dict(sha256=digest(path), executable=bool(path.stat().st_mode & 0o111))
    return result


def tree_identity(files):
    return hashlib.sha256(json.dumps(files, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def snapshot(repository, destination, files):
    destination.mkdir(exist_ok=False)
    for name, expected in files.items():
        original, copied = repository / name, destination / name
        data = original.read_bytes()
        if hashlib.sha256(data).hexdigest() != expected['sha256']:
            raise ValueError('source changed while snapshot was copied')
        copied.parent.mkdir(parents=True, exist_ok=True)
        copied.write_bytes(data)
        copied.chmod(0o755 if expected['executable'] else 0o644)


def verify_snapshot(source, files):
    actual = {str(p.relative_to(source)): dict(sha256=digest(p), executable=bool(p.stat().st_mode & 0o111))
              for p in source.rglob('*') if p.is_file() and not p.is_symlink()}
    if any(p.is_symlink() for p in source.rglob('*')) or actual != files:
        raise ValueError('frozen source tree changed during build')


def compiler_artifact(path):
    artifacts = []
    finished = []
    for line in path.read_text().splitlines():
        event = json.loads(line)
        if event.get('reason') == 'build-finished':
            finished.append(event['success'])
        if event.get('reason') == 'compiler-artifact' and event.get('executable') and event.get('target', {}).get('name') == 'rag-mcp' and 'bin' in event['target']['kind']:
            artifacts.append(event)
    if finished != [True] or len(artifacts) != 1:
        raise ValueError('missing unique successful server compiler artifact')
    artifact = artifacts[0]
    profile = artifact['profile']
    if profile['opt_level'] != '3' or profile['debug_assertions'] or profile['test']:
        raise ValueError('compiler artifact does not establish optimized non-test release profile')
    return artifact


def command_output(command, cwd, env=None):
    return subprocess.check_output(command, cwd=cwd, env=env, text=True, stderr=subprocess.STDOUT).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('repository', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--jobs', type=int, default=2)
    args = parser.parse_args()
    if not 1 <= args.jobs <= 8:
        raise ValueError('jobs must be between 1 and 8')
    repository, root = args.repository.resolve(), args.output.resolve()
    if root.is_relative_to(repository):
        raise ValueError('release staging must be outside the mutable repository')
    root.mkdir(parents=True, exist_ok=False)
    report = dict(contract_version=1, status='snapshotting', started_unix=time.time(),
        repository=str(repository), build_driver_sha256=digest(__file__), snapshot_attempts=[],
        production_binary_replaced=False, production_database_opened=False,
        source_checkout_frozen=False, frozen_build_snapshot=True,
        limitations=['source content identity, not a clean-commit or hermetic bit-reproducibility claim',
            'registry dependencies use locked cached Cargo inputs; external SDK and native toolchain are not fully hashed',
            'successful release build is not runtime/workload/performance qualification'])
    sequence = 0
    def save():
        nonlocal sequence
        temporary = root / f'state-{sequence}.json'
        sequence += 1
        with temporary.open('x') as output:
            output.write(json.dumps(report, indent=2) + '\n')
        os.replace(temporary, root / 'state.json')
    save()
    try:
        source = None
        for attempt in range(3):
            files = inventory(repository)
            candidate = root / f'source-{attempt}'
            try:
                snapshot(repository, candidate, files)
                if inventory(repository) != files:
                    raise ValueError('repository changed across snapshot capture boundaries')
                verify_snapshot(candidate, files)
                source = candidate
                report['snapshot_attempts'].append(dict(attempt=attempt, status='captured', identity=tree_identity(files)))
                break
            except (OSError, ValueError) as error:
                report['snapshot_attempts'].append(dict(attempt=attempt, status='rejected', error=str(error)))
                save()
        if source is None:
            raise ValueError('no stable source snapshot captured')
        (root / 'source-inputs.json').write_text(json.dumps(files, indent=2) + '\n')
        cargo = Path(command_output(['rustup', 'which', 'cargo'], repository)).resolve()
        rustc = Path(command_output(['rustup', 'which', 'rustc'], repository)).resolve()
        rustdoc = Path(command_output(['rustup', 'which', 'rustdoc'], repository)).resolve()
        tools = {name: dict(path=str(path), sha256=digest(path)) for name, path in [('cargo', cargo), ('rustc', rustc), ('rustdoc', rustdoc)]}
        env = {k: v for k, v in os.environ.items() if not k.startswith(('RAG_', 'CARGO_PROFILE_', 'CARGO_BUILD_')) and k not in
               {'CARGO_TARGET_DIR', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTUP_TOOLCHAIN'}}
        env.update(RUSTC=str(rustc), RUSTDOC=str(rustdoc), CARGO_INCREMENTAL='0')
        overrides = ['build.rustflags=[]', 'build.rustc-wrapper=""', 'build.rustc-workspace-wrapper=""',
            'profile.release.opt-level=3', 'profile.release.debug-assertions=false', 'profile.release.lto="thin"',
            'profile.release.codegen-units=1', 'profile.release.strip="debuginfo"']
        command = [str(cargo)]
        for setting in overrides:
            command += ['--config', setting]
        command += ['build', '--release', '--locked', '--offline', '--bin', 'rag-mcp', '--message-format=json-render-diagnostics',
                    '--jobs', str(args.jobs), '--target-dir', str(root / 'target')]
        report.update(status='building', snapshot=str(source), source_tree_sha256=tree_identity(files),
            source_inputs_sha256=digest(root / 'source-inputs.json'), source_files=len(files),
            source_head_at_capture=command_output(['git', 'rev-parse', 'HEAD'], repository),
            tools=tools, rustc_verbose_version=command_output([str(rustc), '-Vv'], source, env),
            cargo_version=command_output([str(cargo), '-V'], source, env),
            host=dict(platform=platform.platform(), machine=platform.machine(), logical_cpus=os.cpu_count()),
            command=command, controlled_environment=dict(RUSTC=str(rustc), RUSTDOC=str(rustdoc), CARGO_INCREMENTAL='0'),
            cargo_lock_sha256=digest(source / 'Cargo.lock'))
        save()
        with (root / 'cargo.jsonl').open('x') as stdout, (root / 'cargo-stderr.log').open('x') as stderr:
            process = subprocess.Popen(command, cwd=source, env=env, stdout=stdout, stderr=stderr)
            report['build_pid'] = process.pid
            save()
            code = process.wait()
        report['cargo_exit_code'] = code
        if code:
            raise RuntimeError(f'Cargo build failed with exit {code}; raw logs retained')
        artifact = compiler_artifact(root / 'cargo.jsonl')
        verify_snapshot(source, files)
        for tool in tools.values():
            if digest(tool['path']) != tool['sha256']:
                raise ValueError('compiler executable changed during build')
        if command_output([str(rustc), '-Vv'], source, env) != report['rustc_verbose_version']:
            raise ValueError('compiler version changed during build')
        executable = Path(artifact['executable'])
        if not executable.resolve().is_relative_to(root / 'target'):
            raise ValueError('compiler executable output escaped owned target directory')
        (root / 'bin').mkdir()
        frozen = root / 'bin/rag-mcp'
        shutil.copy2(executable, frozen)
        manifest = dict(contract_version=1, status='built_verified', profile='release', binary_path=str(frozen),
            binary_sha256=digest(frozen), source_tree_sha256=report['source_tree_sha256'],
            source_inputs_sha256=report['source_inputs_sha256'], cargo_lock_sha256=report['cargo_lock_sha256'],
            compiler_artifact=artifact, command=command, tools=tools,
            rustc_verbose_version=report['rustc_verbose_version'], cargo_version=report['cargo_version'],
            cargo_json_sha256=digest(root / 'cargo.jsonl'), cargo_stderr_sha256=digest(root / 'cargo-stderr.log'),
            source_unchanged_during_build=True, compiler_executables_unchanged=True,
            driver_sha256=report['build_driver_sha256'], runtime_qualified=False, performance_qualified=False,
            production_binary_replaced=False, limitations=report['limitations'])
        if manifest['binary_sha256'] != digest(executable) or digest(__file__) != report['build_driver_sha256']:
            raise ValueError('release artifact or build driver changed during verification')
        with (root / 'release-manifest.json').open('x') as output:
            output.write(json.dumps(manifest, indent=2) + '\n')
        report.update(status='built_verified', binary_sha256=manifest['binary_sha256'], completed_unix=time.time())
        save()
        print(json.dumps(dict(status=report['status'], manifest=str(root / 'release-manifest.json'), binary_sha256=manifest['binary_sha256'])))
    except Exception as error:
        report.update(status='failed', error=dict(type=type(error).__name__, message=str(error)), completed_unix=time.time())
        save()
        raise


if __name__ == '__main__':
    main()
