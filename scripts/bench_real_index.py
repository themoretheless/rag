#!/usr/bin/env python3
"""Owned real-source MCP workloads: commit-inclusive timings, explicit audit mode."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
from mcp_smoke_client import McpClient


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def inputs(source):
    paths = [source / 'Cargo.toml', *sorted((source / 'src').rglob('*.rs'))]
    return {str(p.relative_to(source)): digest(p) for p in paths}


def environment(ignore_pids=()):
    result = subprocess.run(['ps', '-axo', 'pid=,pcpu=,comm='], capture_output=True, text=True, check=True)
    processes = []
    for line in result.stdout.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) != 3:
            continue
        pid, cpu, command = int(parts[0]), float(parts[1]), Path(parts[2]).name
        compiler = command in {'cargo', 'rustc', 'clippy-driver', 'clang', 'clang++', 'cc1', 'ld', 'ninja', 'make'}
        if pid not in {os.getpid(), *ignore_pids} and (compiler or cpu >= 25):
            processes.append(dict(pid=pid, cpu_percent=cpu, command=command, compiler=compiler))
    return dict(blocking_processes=processes, quiet=not processes, sampled_at_unix=time.time(),
                scope='process names and reported CPU only; no thermal/power/whole-machine isolation proof')


def relative_path(source, name):
    source = Path(source).resolve()
    path = Path(name)
    if path.is_absolute() or '..' in path.parts:
        raise ValueError('unsafe workload path')
    target = source / path
    if target.is_symlink() or not target.is_file() or not target.resolve().is_relative_to(source):
        raise ValueError('workload file must be a regular owned source input')
    return target


def apply(source, phase):
    changed = []
    for edit in phase['edits']:
        path = relative_path(source, edit['path'])
        previous = path.read_bytes()
        if 'append' in edit:
            if 'before' in edit or 'after' in edit:
                raise ValueError('append and replace edits are exclusive')
            updated = previous + edit['append'].encode()
        else:
            before, after = edit['before'].encode(), edit['after'].encode()
            if not before or previous.count(before) != 1:
                raise ValueError('workload replacement must match exactly once')
            updated = previous.replace(before, after, 1)
        if updated == previous:
            raise ValueError('workload must change source bytes')
        path.write_bytes(updated)
        changed.append(dict(path=edit['path'], before_sha256=hashlib.sha256(previous).hexdigest(), after_sha256=digest(path)))
    return changed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('binary', type=Path)
    parser.add_argument('source', type=Path)
    parser.add_argument('recipe', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--mode', choices=['incremental', 'full_binding_control'], default='incremental')
    parser.add_argument('--qualification-only', action='store_true', help='compiler and equivalence audits; timings are diagnostic')
    parser.add_argument('--build-manifest', type=Path, help='release artifact manifest with profile=release and binary_sha256')
    args = parser.parse_args()
    binary, origin = args.binary.resolve(), args.source.resolve()
    recipe = json.loads(args.recipe.read_text())
    if recipe['contract_version'] != 1:
        raise ValueError('unsupported workload recipe')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.output.exists():
        raise FileExistsError('refuse to replace an existing benchmark report')
    if origin.is_symlink() or any(p.is_symlink() for p in origin.rglob('*')):
        raise ValueError('original workload snapshot must not contain symlinks')
    before_inputs = inputs(origin)
    if before_inputs != recipe['expected_inputs']:
        raise ValueError('pinned workload source/config hashes differ')
    names = [p['name'] for p in recipe['phases']]
    if len(set(names)) != len(names) or set(names) & {'cold_ingest_and_index', 'warm_noop'}:
        raise ValueError('workload phase names must be unique')
    if any(not p['edits'] or any(e['path'] not in before_inputs for e in p['edits']) for p in recipe['phases']):
        raise ValueError('workload edits must target declared indexed inputs')
    binary_hash = digest(binary)
    build = json.loads(args.build_manifest.read_text()) if args.build_manifest else None
    release_declared = bool(build and build.get('profile') == 'release' and build.get('binary_sha256') == binary_hash)
    preflight = environment()
    report = dict(contract_version=1, status='pending', qualification_only=args.qualification_only,
        binary_sha256=binary_hash, runner_sha256=digest(__file__), client_sha256=digest(Path(__file__).with_name('mcp_smoke_client.py')), build_manifest=build, release_manifest_identity_matches=release_declared,
        recipe_sha256=digest(args.recipe), recipe=recipe, source_inputs=before_inputs,
        mode=args.mode, preflight=preflight, controls=[], records=[], correctness_audits=[], compiler_checks=[],
        production_database_used=False, performance_qualified=False,
        limitations=['small real-library workload; no representative scale claim',
            'mock embeddings (16 dimensions), local FTS/document writes and code graph; no model cost',
            'startup, source-copy, compiler and equivalence audits excluded from timing',
            'sampled worker RSS is a lower bound, not exact physical operation peak',
            'full binding control is an internal algorithm control, not a competitor',
            'manifest_metadata changes metadata, not Cargo feature/dependency semantics',
            'release profile is supplied by an external manifest; this runner checks identity, not compiler provenance'])

    def save():
        with args.output.open('x') as destination:
            destination.write(json.dumps(report, indent=2) + '\n')

    if not args.qualification_only and (not preflight['quiet'] or not release_declared):
        report.update(status='rejected_environment_or_build', rejection_reasons=[
            *(['competing process activity'] if not preflight['quiet'] else []),
            *(['matching release artifact manifest required'] if not release_declared else [])])
        save()
        print(json.dumps(dict(status=report['status'], reasons=report['rejection_reasons'])))
        raise SystemExit(3)
    try:
        with tempfile.TemporaryDirectory(prefix='rag-real-index-bench-') as temporary:
            root = Path(temporary).resolve()
            source = root / 'sources'
            shutil.copytree(origin, source, symlinks=True, ignore=shutil.ignore_patterns('.git', 'target'))
            if any(p.is_symlink() for p in source.rglob('*')):
                raise ValueError('benchmark snapshot must not contain symlinks')
            assert inputs(source) == before_inputs
            compiler_env = {k: v for k, v in os.environ.items() if k not in
                ('CARGO_TARGET_DIR', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET')}
            compiler_env['CARGO_TARGET_DIR'] = str(root / 'compiler-target')
            try:
                with McpClient(binary, root, source) as client:
                    def call(name, arguments):
                        result = client.request('tools/call', dict(name=name, arguments=arguments), timeout=300)
                        assert not result.get('isError'), result
                        return json.loads(next(c['text'] for c in result['content'] if c['type'] == 'text'))

                    def audit(label):
                        if not args.qualification_only:
                            return
                        command = ['cargo', 'check', '--offline', *recipe.get('compiler_arguments', ['--lib']), '--manifest-path', str(source / 'Cargo.toml')]
                        compiled = subprocess.run(command, cwd=source, env=compiler_env, capture_output=True, text=True)
                        report['compiler_checks'].append(dict(case=label, command=command, exit_code=compiled.returncode,
                            stdout=compiled.stdout, stderr=compiled.stderr))
                        assert compiled.returncode == 0, compiled.stderr
                        result = call('index_code', dict(force_full_resolution=True))
                        report['correctness_audits'].append(dict(case=label, result=result))
                        assert result.get('resolution', {}).get('equivalent_to_existing') is True, result
                        assert not result['revision_created'], result

                    def measure(label, changed, ordinary_noop=False):
                        control = environment(ignore_pids=[client.process.pid])
                        control_record = dict(case=label, before=control)
                        report['controls'].append(control_record)
                        if not args.qualification_only and not control['quiet']:
                            raise RuntimeError('competing activity detected before measured phase')
                        samples = []
                        stop = threading.Event()
                        def sample():
                            while not stop.is_set():
                                observed = subprocess.run(['ps', '-o', 'rss=', '-p', str(client.process.pid)], capture_output=True, text=True)
                                if observed.returncode == 0 and observed.stdout.strip().isdigit():
                                    samples.append(int(observed.stdout.strip()) * 1024)
                                stop.wait(.05)
                        watcher = threading.Thread(target=sample)
                        watcher.start()
                        steps = []
                        record = dict(case=label, steps=steps, inputs=inputs(source), ordinary_noop=ordinary_noop, environment=control)
                        report['records'].append(record)
                        started = time.perf_counter_ns()
                        try:
                            for relative in changed:
                                substart = time.perf_counter_ns()
                                value = call('ingest_file', dict(path=str(source / relative), wing='bench'))
                                steps.append(dict(operation='ingest_file', path=relative, ms=(time.perf_counter_ns()-substart)/1e6, result=value))
                            options = dict(force_full_resolution=True) if args.mode == 'full_binding_control' and not ordinary_noop else {}
                            substart = time.perf_counter_ns()
                            indexed = call('index_code', options)
                            steps.append(dict(operation='index_code', arguments=options, ms=(time.perf_counter_ns()-substart)/1e6, result=indexed))
                        finally:
                            elapsed = (time.perf_counter_ns()-started)/1e6
                            stop.set()
                            watcher.join()
                            record.update(end_to_end_mcp_ms=elapsed,
                                sampled_worker_peak_rss_bytes=max(samples) if samples else None,
                                rss_samples=len(samples), rss_sample_period_seconds=.05)
                        post = environment(ignore_pids=[client.process.pid])
                        record['environment_after'] = post
                        control_record['after'] = post
                        if not args.qualification_only and not post['quiet']:
                            raise RuntimeError('competing activity detected after measured phase')
                        assert indexed['indexed'] == len(set(changed)), indexed
                        assert indexed['unchanged'] == len(before_inputs)-len(set(changed)), indexed
                        assert bool(indexed['revision_created']) == bool(changed), indexed
                        audit(label)

                    measure('cold_ingest_and_index', list(before_inputs))
                    measure('warm_noop', [], ordinary_noop=True)
                    for phase in recipe['phases']:
                        mutations = apply(source, phase)
                        measure(phase['name'], list(dict.fromkeys(m['path'] for m in mutations)))
                        report['records'][-1]['mutations'] = mutations
                    report['index_status'] = call('index_status', dict(project='bench'))
            finally:
                stderr = root / 'stderr.log'
                report['isolated_stderr'] = stderr.read_text() if stderr.exists() else None
        assert inputs(origin) == before_inputs, 'original real source was modified'
        assert digest(binary) == binary_hash, 'binary changed during run'
        assert digest(__file__) == report['runner_sha256'], 'benchmark runner changed during run'
    except Exception as error:
        report.update(status='failed_qualification' if args.qualification_only else 'rejected_run',
            error=dict(type=type(error).__name__, message=str(error)),
            original_source_unchanged=inputs(origin) == before_inputs,
            binary_unchanged=digest(binary) == binary_hash)
        save()
        raise
    report.update(status='qualified_workloads' if args.qualification_only else 'measured_unpaired',
                  original_source_unchanged=True, binary_unchanged=True,
                  performance_qualified=False)
    save()
    print(json.dumps(dict(status=report['status'], cases=len(report['records']),
                         compiler_checks=len(report['compiler_checks']), equivalence_checks=len(report['correctness_audits']))))


if __name__ == '__main__':
    main()
