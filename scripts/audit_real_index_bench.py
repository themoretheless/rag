#!/usr/bin/env python3
"""Verify real workload receipts and audit evidence; never turns debug times into wins."""
import argparse
import json
from pathlib import Path
import shutil
import sys
import tempfile
from bench_real_index import apply, digest, inputs
from verify_index_release import verify as verify_release


def audit(folder, origin, recipe_path, release_manifest=None):
    recipe = json.loads(recipe_path.read_text())
    runs = [json.loads((folder / f'accepted-{mode}.json').read_text()) for mode in ('incremental', 'full-binding')]
    assert [r['mode'] for r in runs] == ['incremental', 'full_binding_control']
    release = None
    if release_manifest is not None:
        manifest = json.loads(release_manifest.read_text())
        release = verify_release(release_manifest, Path(manifest['binary_path']))
        assert all(r['build_manifest'] == manifest and r['binary_sha256'] == release['binary_sha256'] for r in runs)
    source = inputs(origin)
    assert source == recipe['expected_inputs']
    runner_hash = digest(Path(__file__).with_name('bench_real_index.py'))
    phases = ['cold_ingest_and_index', 'warm_noop', *[p['name'] for p in recipe['phases']]]
    expected = []
    with tempfile.TemporaryDirectory(prefix='rag-workload-replay-') as temporary:
        owned = Path(temporary).resolve()
        for name in source:
            target = owned / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(origin / name, target)
        expected.extend([(source, list(source), None), (source, [], None)])
        for phase in recipe['phases']:
            receipts = apply(owned, phase)
            expected.append((inputs(owned), list(dict.fromkeys(r['path'] for r in receipts)), receipts))
    for run in runs:
        assert run['status'] == 'qualified_workloads' and run['qualification_only']
        assert not run['performance_qualified'] and not run['production_database_used']
        assert run['runner_sha256'] == runner_hash
        assert run['client_sha256'] == digest(Path(__file__).with_name('mcp_smoke_client.py'))
        assert run['recipe_sha256'] == digest(recipe_path) and run['recipe'] == recipe
        assert run['original_source_unchanged'] and run['binary_unchanged']
        assert run['source_inputs'] == source
        assert [r['case'] for r in run['records']] == phases
        assert [r['case'] for r in run['compiler_checks']] == phases
        assert all(r['exit_code'] == 0 for r in run['compiler_checks'])
        assert [r['case'] for r in run['correctness_audits']] == phases
        assert len(run['controls']) == len(phases)
        for record, full, (state, changed, receipts) in zip(run['records'], run['correctness_audits'], expected):
            assert record['inputs'] == state
            if receipts is not None:
                assert record['mutations'] == receipts
            assert [s['path'] for s in record['steps'][:-1]] == changed
            assert all(s['operation'] == 'ingest_file' for s in record['steps'][:-1])
            index = record['steps'][-1]
            assert index['operation'] == 'index_code'
            assert index['result']['indexed'] == len(changed)
            assert index['result']['unchanged'] == len(source)-len(changed)
            assert bool(index['result']['revision_created']) == bool(changed)
            args = {} if run['mode'] == 'incremental' or record['case'] == 'warm_noop' else dict(force_full_resolution=True)
            assert index['arguments'] == args
            assert record['ordinary_noop'] == (record['case'] == 'warm_noop')
            assert record['end_to_end_mcp_ms'] >= sum(s['ms'] for s in record['steps'])
            assert full['result']['resolution']['equivalent_to_existing'] is True
            assert not full['result']['revision_created']
            assert full['result']['index_revision'] == index['result']['index_revision']
        assert run['records'][0]['steps'][-1]['result']['index_revision'] == run['records'][1]['steps'][-1]['result']['index_revision']
        assert not run['index_status']['parse_errors']
        assert not run['index_status']['stale_documents']
        assert not run['index_status']['unindexed_documents']
    for field in ['binary_sha256', 'runner_sha256', 'client_sha256', 'recipe_sha256', 'source_inputs']:
        assert runs[0][field] == runs[1][field]
    rejected = json.loads((folder / 'rejected-performance-final.json').read_text())
    assert rejected['status'] == 'rejected_environment_or_build'
    assert not rejected['performance_qualified'] and not rejected['records']
    assert rejected['runner_sha256'] == runner_hash
    assert 'matching release artifact manifest required' in rejected['rejection_reasons']
    failure = json.loads((folder / 'failed-compiler-audit-final.json').read_text())
    assert failure['status'] == 'failed_qualification' and not failure['performance_qualified']
    assert failure['runner_sha256'] == runner_hash
    assert failure['original_source_unchanged'] and failure['binary_unchanged']
    assert len(failure['records']) == len(failure['compiler_checks']) == 1
    assert failure['compiler_checks'][0]['exit_code'] != 0 and failure['compiler_checks'][0]['stderr']
    assert 'isolated_stderr' in failure and failure['error'] and not failure['correctness_audits']
    assert inputs(origin) == source
    return dict(contract_version=1, status='qualified_workloads', binary_sha256=runs[0]['binary_sha256'],
        runner_sha256=runner_hash, recipe_sha256=digest(recipe_path), modes=2, phases_per_mode=len(phases),
        compiler_checks=sum(len(r['compiler_checks']) for r in runs),
        full_equivalence_audits=sum(len(r['correctness_audits']) for r in runs),
        identical_phase_inputs_and_mutations=True, ordinary_noop_both_modes=True,
        mutation_input_counts=[len(v[1]) for v in expected],
        commit_inclusive_ingest_and_index=True, original_source_unchanged=True,
        rejected_performance_report_retained=True, failed_compiler_audit_report_retained=True,
        release_artifact_verified=release is not None, release_verification=release,
        performance_qualified=False, release_speed_qualified=False, competitor_comparison=False,
        limitations=['workflow qualification includes compiler/equivalence audits; timings diagnostic only',
            'single small real repository; no representative timing/RSS or scale conclusion',
            'full binding audit proves cache equivalence, not independent semantic accuracy'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('folder', type=Path)
    parser.add_argument('origin', type=Path)
    parser.add_argument('recipe', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--release-manifest', type=Path)
    args = parser.parse_args()
    result = audit(args.folder, args.origin, args.recipe, args.release_manifest)
    with args.output.open('x') as destination:
        destination.write(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
