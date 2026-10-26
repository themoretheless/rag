#!/usr/bin/env python3
"""Audit additive external-negative labels without changing the positive baseline."""
import collections
import hashlib
import json
from pathlib import Path
import sys
from qualify_rust_corpus import metrics
from scip_binding_labels import labels


def key(case):
    return case['path'], tuple(case['range']), case['symbol']


def summarize(baseline, expanded):
    repositories = {}
    original_cases = []
    negatives = []
    binaries = set()
    helpers = set()
    for name in ('itoa', 'ryu', 'memchr'):
        old = baseline / f'controlled-{name}-after'
        new = expanded / f'controlled-{name}-after'
        before = json.loads((old / 'report.json').read_text())
        after = json.loads((new / 'report.json').read_text())
        oracle = json.loads((new / 'oracle-labels.json').read_text())
        cases = json.loads((new / 'scored-cases.json').read_text())['cases']
        prior = json.loads((old / 'scored-cases.json').read_text())['cases']
        assert after['status'] == 'measured' and after['labelled_native_mcp_parity']
        assert after['parity_sites'] == len(cases) == len(oracle['cases'])
        assert oracle['contract_version'] == after['oracle_contract_version'] == 2
        assert after['metrics'] == metrics(cases)
        assert after['label_cases_sha256'] == hashlib.sha256(json.dumps(oracle['cases'], sort_keys=True).encode()).hexdigest()
        assert [{k: c[k] for k in oracle['cases'][i]} for i, c in enumerate(cases)] == oracle['cases']
        for control in ('input_sha256', 'oracle_sha256', 'binary_sha256', 'native_capture_sha256'):
            assert before[control] == after[control], (name, control)
        # Recompute with the final adapter, including its ambiguity guards.
        source = Path(after['source'])
        current = labels(source.parent / (name + '.scip'), source,
                         source.parent / 'ast' / name, include_external_descriptors=True)
        assert current == oracle, name
        indexed = {key(c): c for c in cases}
        assert len(indexed) == len(cases), 'duplicate expanded oracle anchors'
        anchors = collections.Counter((c['path'], tuple(c['range'])) for c in cases)
        assert max(anchors.values(), default=0) == 1, 'ambiguous source anchor'
        preserved = []
        for previous in prior:
            same = indexed[key(previous)]
            for field in ('path', 'range', 'symbol', 'kinds', 'syntax_class', 'expected',
                          'observed_definitions', 'observations', 'true_positive', 'false_positive', 'false_negative', 'exact'):
                assert previous[field] == same[field], (name, field, key(previous))
            preserved.append(same)
        prior_keys = {key(c) for c in prior}
        extra = [c for c in cases if key(c) not in prior_keys]
        assert all(not c['expected'] and not c['kinds'] and c['syntax_class'] == 'direct_call'
                   and c['callable_evidence'].startswith('SCIP Method descriptor') for c in extra)
        assert metrics(preserved) == before['metrics']
        negatives.extend(extra)
        original_cases.extend(preserved)
        binaries.add(after['binary_sha256'])
        helpers.add(after['native_capture_sha256'])
        repositories[name] = dict(original_metrics=metrics(preserved), external_negative_sites=len(extra),
            external_negative_evidence=after['external_negative_evidence'], expanded_metrics=after['metrics'],
            baseline_artifact_sha256={str(old / f): hashlib.sha256((old / f).read_bytes()).hexdigest()
                for f in ('report.json', 'scored-cases.json', 'oracle-labels.json')})
    assert len(binaries) == len(helpers) == 1
    return dict(contract_version=1, status='measured', original_cases_unchanged=True,
        original_observations_and_results_unchanged=True, identical_source_config_and_binary=True,
        native_mcp_parity=True, final_adapter_oracle_identity=True,
        binary_sha256=next(iter(binaries)), native_helper_sha256=next(iter(helpers)),
        repositories=repositories, original_metrics=metrics(original_cases),
        external_negatives=dict(sites=len(negatives),
            with_extracted_observation=sum(bool(c['observations']) for c in negatives),
            without_extracted_observation=sum(not c['observations'] for c in negatives),
            false_local_candidates=sum(c['false_positive'] for c in negatives),
            sites_with_false_local_candidate=sum(bool(c['false_positive']) for c in negatives),
            packages=dict(collections.Counter(c['symbol'].split(' ')[2] for c in negatives)),
            missing_observations=[dict(path=c['path'], range=c['range'], symbol=c['symbol']) for c in negatives if not c['observations']]),
        limitations=['additive external-package negative labels; not an improvement in positive recall',
            'no local candidate is not proof of external resolution or extraction',
            'core/alloc references only; no broad third-party negative qualification',
            'original RA error/exclusion/configuration limits remain; no compiler/runtime proof',
            'no release performance or competitor-superiority evidence'])


if __name__ == '__main__':
    result = summarize(Path(sys.argv[1]), Path(sys.argv[2]))
    with Path(sys.argv[3]).open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')
    print(json.dumps(dict(original=result['original_metrics'], external=result['external_negatives'])))
