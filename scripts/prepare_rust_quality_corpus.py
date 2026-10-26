#!/usr/bin/env python3
"""Pinned public repositories, explicit rustc/build-script cfg, independent SCIP."""
import argparse
import collections
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile

PINS = {
    "itoa": ("dtolnay/itoa", "1577ed901354d0d7448ac162328f9dbf5183124c"),
    "ryu": ("dtolnay/ryu", "75f9e707e77c89717ffb9c2a01fe2c28a016cc82"),
    "memchr": ("BurntSushi/memchr", "bd6068c30e9074a90c285e47912fa0b047d07597"),
}


def run(args, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed: {result.stderr[-3000:]}")
    return result


def cfg(items):
    flags = set()
    values = collections.defaultdict(set)
    for item in items:
        if "=" in item:
            key, value = item.split("=", 1)
            values[key].add(json.loads(value))
        else:
            flags.add(item)
    return flags, values


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("rust_analyzer", type=Path)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--repositories", nargs="+", choices=list(PINS), default=list(PINS))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    analyzer = args.rust_analyzer.resolve()
    env = {k: v for k, v in os.environ.items() if k not in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR"}}
    rustc = run(["rustc", "-vV"], env=env).stdout
    host = next(line[6:] for line in rustc.splitlines() if line.startswith("host: "))
    compiler_cfg = run(["rustc", "--print", "cfg", "--target", host], env=env).stdout.splitlines()
    config = dict(cargo=dict(allTargets=False, cfgs=["!test", "!miri"], features=[], target=host), procMacro=dict(enable=False))
    config_path = output / "oracle-config.json"
    config_path.write_text(json.dumps(config, indent=2) + "\n")
    manifest = dict(contract_version=1, rustc=rustc, host=host, rustc_cfg=compiler_cfg,
                    analyzer_version=run([str(analyzer), "--version"]).stdout.strip(),
                    analyzer_sha256=hashlib.sha256(analyzer.read_bytes()).hexdigest(), repositories={},
                    scope="default-feature library target; test/miri disabled; explicit compiler/build-script cfg",
                    limitations=["SCIP is independent language-server evidence, not compiler/runtime proof", "custom metadata overlay is recorded; Rust source remains pinned", "three library crates are not representative of all Rust ecosystems"])
    for name in args.repositories:
        repository, commit = PINS[name]
        cache = args.cache.resolve() / name
        if not (cache / ".git").exists():
            run(["git", "clone", "--filter=blob:none", "--no-checkout", f"https://github.com/{repository}.git", str(cache)])
        run(["git", "-C", str(cache), "cat-file", "-e", commit + "^{commit}"])
        archive = subprocess.check_output(["git", "-C", str(cache), "archive", commit])
        root = output / name
        root.mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(root, filter="data")
        original = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(root.rglob("*")) if p.is_file()}
        metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(root / "Cargo.toml")], env=env).stdout)
        package_id = next(p["id"] for p in metadata["packages"] if Path(p["manifest_path"]).resolve() == root / "Cargo.toml")
        checked = run(["cargo", "check", "--lib", "--target", host, "--message-format=json", "--manifest-path", str(root / "Cargo.toml"), "--target-dir", str(output / "cargo-target" / name)], env=env)
        (output / f"{name}-cargo-check.jsonl").write_text(checked.stdout)
        (output / f"{name}-cargo-check.log").write_text(checked.stderr)
        emitted = [item for line in checked.stdout.splitlines() for item in [json.loads(line)] if item.get("reason") == "build-script-executed" and item["package_id"] == package_id]
        flags, values = cfg(compiler_cfg + [value for item in emitted for value in item["cfgs"]])
        flags.discard("test")
        flags.discard("miri")
        overlay = ('\n[package.metadata.rag.graph]\nversion=1\ntarget_kind="lib"\ndefault_features=true\nfeatures=[]\n'
                   + 'target_triple=' + json.dumps(host) + '\ncfg_flags=' + json.dumps(sorted(flags))
                   + '\n[package.metadata.rag.graph.cfg_values]\n'
                   + ''.join(key + '=' + json.dumps(sorted(value)) + '\n' for key, value in sorted(values.items())))
        manifest_path = root / "Cargo.toml"
        manifest_path.write_text(manifest_path.read_text() + overlay)
        (output / f"{name}-overlay.toml").write_text(overlay)
        scip = output / f"{name}.scip"
        indexed = run([str(analyzer), "scip", str(root), "--config-path", str(config_path), "--output", str(scip), "--num-threads", "2"], env=env)
        (output / f"{name}-oracle.log").write_text(indexed.stdout + indexed.stderr)
        for path in sorted((root / "src").rglob("*.rs")):
            parsed = run([str(analyzer), "parse", "--json"], input=path.read_bytes().decode("utf-8"), env=env)
            destination = output / "ast" / name / (str(path.relative_to(root)) + ".json")
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(parsed.stdout)
        manifest["repositories"][name] = dict(repository=repository, commit=commit, original_sha256=original,
            overlay_sha256=hashlib.sha256(overlay.encode()).hexdigest(), source_manifest_sha256=hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
            build_script_cfgs=[value for item in emitted for value in item["cfgs"]], cargo_check_exit=checked.returncode,
            oracle_exit=indexed.returncode, oracle_log_errors=indexed.stderr.count("ERROR"), scip_sha256=hashlib.sha256(scip.read_bytes()).hexdigest())
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(json.dumps(dict(repository=name, commit=commit, prepared=True)), flush=True)


if __name__ == "__main__":
    main()
