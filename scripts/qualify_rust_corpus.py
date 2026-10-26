#!/usr/bin/env python3
"""Independent SCIP definition correspondence, native capture and real MCP parity."""
import collections
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from mcp_smoke_client import McpClient
from scip_binding_labels import labels


def site(path, line, column):
    return (path, line, column)


def metrics(cases):
    tp = sum(c["true_positive"] for c in cases)
    fp = sum(c["false_positive"] for c in cases)
    fn = sum(c["false_negative"] for c in cases)
    return dict(labelled_sites=len(cases), local_positive_sites=sum(bool(c["expected"]) for c in cases),
                external_negative_sites=sum(not c["expected"] for c in cases),
                true_positive=tp, false_positive=fp, false_negative=fn,
                precision=tp / (tp + fp) if tp + fp else None,
                recall=tp / (tp + fn) if tp + fn else None,
                exact_sites=sum(c["exact"] for c in cases),
                exact_site_rate=sum(c["exact"] for c in cases) / len(cases) if cases else None)


def evaluate(native, oracle, source):
    symbols = {s["id"]: s for s in native["symbols"]}
    positions = {}
    for key, symbol in symbols.items():
        path = Path(symbol["path"])
        relative = str(path.relative_to(source))
        data = path.read_bytes()
        fragment = data[symbol["start_byte"]:symbol["end_byte"]]
        pattern = rb"\b(?:fn|struct|enum|type|trait|const|static)\s+(" + re.escape(symbol["name"].encode()) + rb")(?=[\s<(;:{])"
        match = re.search(pattern, fragment)
        offset = symbol["start_byte"] + match.start(1) if match else symbol["start_byte"]
        line = data[:offset].count(b"\n")
        column = offset - (data.rfind(b"\n", 0, offset) + 1)
        positions[key] = (relative, line, column)
    observed = collections.defaultdict(set)
    observations = collections.defaultdict(list)
    for edge in native["bound_references"]:
        reference = edge["reference"]
        relative = str(Path(reference["path"]).relative_to(source))
        observed[site(relative, reference["line"] - 1, reference["column"] - 1)].update(edge["candidates"])
        observations[site(relative, reference["line"] - 1, reference["column"] - 1)].append(dict(expression=reference["expression"], is_call=reference["is_call"], basis=edge["resolution_basis"], resolution=edge["resolution"]))
    result = []
    for case in oracle["cases"]:
        key = site(case["path"], case["range"][0], case["range"][1])
        expected = {(t["path"], t["range"][0], t["range"][1]) for t in case["expected"]}
        candidates = sorted(observed[key])
        actual = {positions.get(c, ("<missing_native_target>", c)) for c in candidates}
        hit, wrong, missed = len(actual & expected), len(actual - expected), len(expected - actual)
        result.append(dict(**case, observed_candidates=candidates, observed_definitions=sorted(actual),
                           observations=observations[key], expected_definition_extracted=expected.issubset(set(positions.values())),
                           true_positive=hit, false_positive=wrong, false_negative=missed, exact=actual == expected))
    return dict(cases=result, metrics=metrics(result),
                metrics_by_syntax={kind: metrics([c for c in result if c["syntax_class"] == kind])
                                   for kind in sorted({c["syntax_class"] for c in result})}, symbols=symbols)


def main():
    arguments = sys.argv[1:]
    external_descriptors = bool(arguments and arguments[-1] == "--external-descriptors")
    if external_descriptors:
        arguments.pop()
    if len(arguments) != 5:
        raise ValueError("expected binary capture source scip output [--external-descriptors]")
    binary, capture, source, scip, output = map(lambda p: Path(p).resolve(), arguments)
    output.mkdir(parents=True, exist_ok=False)
    syntax = source.parent / "ast" / source.name
    oracle = labels(scip, source, syntax if syntax.exists() else None, include_external_descriptors=external_descriptors)
    (output / "oracle-labels.json").write_text(json.dumps(oracle, indent=2) + "\n")
    with tempfile.TemporaryDirectory(prefix="rag-code-quality-") as temporary:
        root = Path(temporary).resolve()
        (root / ".qualification-owned").write_text("rag code quality temporary store\n")
        with McpClient(binary, root, source) as client:
            start = time.perf_counter()
            inputs = [source / "Cargo.toml", *sorted((source / "src").rglob("*.rs"))]
            input_sha256 = {str(p.relative_to(source)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}
            ingests = []
            for path in inputs:
                ingests.append(client.call("ingest_file", dict(path=str(path), wing="corpus")))
            sync = dict(input_recipe="Cargo.toml plus every src/**/*.rs; default-feature library workload", inputs=[str(p.relative_to(source)) for p in inputs], ingested=len(ingests), code_index=client.call("index_code", {}))
            sync_ms = (time.perf_counter() - start) * 1000
            status = client.call("index_status", dict(project="corpus"))
            contexts = [c for c in status["rust_build_contexts"] if Path(c["manifest_path"]) == source / "Cargo.toml"]
            if len(contexts) != 1 or not contexts[0].get("available"):
                (output / "rejected-configuration.json").write_text(json.dumps(dict(status=status, sync=sync), indent=2) + "\n")
                raise AssertionError("configuration not admitted; no quality score accepted")
        native_path = root / "native.json"
        native_run = subprocess.run([str(capture), str(root), str(native_path)], capture_output=True, text=True)
        assert native_run.returncode == 0, native_run.stdout + native_run.stderr
        native = json.loads(native_path.read_text())
        scored = evaluate(native, oracle, source)
        symbols = scored.pop("symbols")
        # Query by target-definition file; fall back to exact target IDs whenever
        # the existing 500-result MCP bound would truncate a response.
        target_ids = set(symbols)
        target_paths = sorted({symbols[c]["path"] for c in target_ids})
        mcp = collections.defaultdict(set)
        requests = []
        with McpClient(binary, root, source) as client:
            if not target_paths:
                response = client.call("references", dict(project="corpus", reference_mode="bound", limit=500))
                requests.append(dict(arguments=dict(project="corpus", reference_mode="bound", limit=500), total=response["total"], truncated=response["truncated"]))
                assert not response["truncated"], "empty native target set but truncated MCP binding facts"
                for ref in response["items"]:
                    relative = str(Path(ref["path"]).relative_to(source))
                    mcp[site(relative, ref["line"] - 1, ref["column"] - 1)].update(ref["candidates"])
            for path in target_paths:
                arguments = dict(project="corpus", path=path, reference_mode="bound", limit=500)
                response = client.call("references", arguments)
                requests.append(dict(arguments=arguments, total=response["total"], truncated=response["truncated"]))
                responses = [response]
                if response["truncated"]:
                    responses = []
                    for key in sorted(c for c in target_ids if symbols[c]["path"] == path):
                        arguments = dict(project="corpus", symbol=key, reference_mode="bound", limit=500)
                        response = client.call("references", arguments)
                        requests.append(dict(arguments=arguments, total=response["total"], truncated=response["truncated"]))
                        assert not response["truncated"], "exact-symbol MCP response exceeds current retrieval bounds"
                        responses.append(response)
                for response in responses:
                    for ref in response["items"]:
                        relative = str(Path(ref["path"]).relative_to(source))
                        mcp[site(relative, ref["line"] - 1, ref["column"] - 1)].update(ref["candidates"])
        parity = []
        for case in scored["cases"]:
            key = site(case["path"], case["range"][0], case["range"][1])
            candidates = sorted(mcp[key])
            assert candidates == case["observed_candidates"], (case, candidates)
            parity.append(dict(path=key[0], line=key[1], column=key[2], candidates=candidates))
        (output / "scored-cases.json").write_text(json.dumps(scored, indent=2) + "\n")
        assert input_sha256 == {str(p.relative_to(source)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inputs}, "source changed during qualification"
        external_cases = [c for c in scored["cases"] if not c["expected"]]
        negative_evidence = dict(
            sites=len(external_cases),
            with_extracted_observation=sum(bool(c["observations"]) for c in external_cases),
            without_extracted_observation=sum(not c["observations"] for c in external_cases),
            with_false_local_candidate=sum(bool(c["false_positive"]) for c in external_cases),
            limitation="absence of a local candidate does not prove external-call extraction or external target resolution")
        report = dict(external_negative_evidence=negative_evidence, oracle_contract_version=oracle["contract_version"], status="measured", metrics=scored["metrics"], metrics_by_syntax=scored["metrics_by_syntax"], oracle_sha256=oracle["scip_sha256"],
                      input_sha256=input_sha256, label_cases_sha256=hashlib.sha256(json.dumps(oracle["cases"], sort_keys=True).encode()).hexdigest(),
                      binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), native_capture_sha256=hashlib.sha256(capture.read_bytes()).hexdigest(),
                      native_snapshot_sha256=hashlib.sha256(native_path.read_bytes()).hexdigest(), source=str(source),
                      index_status=status, sync=sync, sync_mcp_round_trip_ms=sync_ms, mcp_requests=requests,
                      labelled_native_mcp_parity=True, parity_sites=len(parity),
                      native_counts={k: len(native[k]) for k in ["symbols", "references", "bound_references"]},
                      oracle_exclusions=oracle["excluded"], production_database_touched=False,
                      limitations=["SCIP correspondence is not compiler/runtime proof", "src callable reference subset, not whole-program recall", "ambiguous/generated oracle anchors excluded with counts", "uncontrolled debug sync timing is diagnostic only"])
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(dict(output=str(output), metrics=report["metrics"], native_mcp_parity=True)), flush=True)


if __name__ == "__main__":
    main()
