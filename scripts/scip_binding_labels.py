#!/usr/bin/env python3
"""SCIP protobuf facts only: no dependency on rag's parser or resolver.

Wire fields follow the recorded scip.proto snapshot. Unknown fields are skipped;
truncation, unsupported encodings and conflicting range forms are rejected.
"""
import collections
import hashlib
import json
from pathlib import Path
import re
import tomllib


def varint(data, pos):
    value = 0
    for shift in range(0, 70, 7):
        if pos >= len(data):
            raise ValueError("truncated protobuf varint")
        byte = data[pos]
        pos += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, pos
    raise ValueError("oversized protobuf varint")


def fields(data):
    result = collections.defaultdict(list)
    pos = 0
    while pos < len(data):
        key, pos = varint(data, pos)
        number, wire = key >> 3, key & 7
        if not number:
            raise ValueError("zero protobuf field")
        if wire == 0:
            value, pos = varint(data, pos)
        elif wire == 2:
            size, pos = varint(data, pos)
            if pos + size > len(data):
                raise ValueError("truncated protobuf bytes")
            value, pos = data[pos:pos + size], pos + size
        elif wire in (1, 5):
            size = 8 if wire == 1 else 4
            if pos + size > len(data):
                raise ValueError("truncated fixed protobuf field")
            value, pos = data[pos:pos + size], pos + size
        else:
            raise ValueError("unsupported protobuf wire type")
        result[number].append((wire, value))
    return result


def integer(message, number, default=0):
    values = message.get(number, [])
    if not values:
        return default
    if len(values) != 1 or values[0][0] != 0:
        raise ValueError("conflicting integer field")
    return values[0][1]


def string(message, number, default=""):
    values = message.get(number, [])
    if not values:
        return default
    if len(values) != 1 or values[0][0] != 2:
        raise ValueError("conflicting string field")
    return values[0][1].decode("utf-8")


def children(message, number):
    for wire, data in message.get(number, []):
        if wire != 2:
            raise ValueError("invalid message wire type")
        yield fields(data)


def source_range(occurrence):
    old = []
    for wire, data in occurrence.get(1, []):
        if wire == 0:
            old.append(data)
        elif wire == 2:
            pos = 0
            while pos < len(data):
                value, pos = varint(data, pos)
                old.append(value)
        else:
            raise ValueError("invalid packed range")
    if len(old) == 3:
        old = [old[0], old[1], old[0], old[2]]
    typed = []
    for field, single in [(8, True), (9, False)]:
        for message in children(occurrence, field):
            typed.append([integer(message, 1), integer(message, 2), integer(message, 1) if single else integer(message, 3), integer(message, 3) if single else integer(message, 4)])
    if len(typed) > 1 or (typed and old and old != typed[0]):
        raise ValueError("conflicting SCIP ranges")
    result = typed[0] if typed else old
    if len(result) != 4 or min(result) < 0 or max(result) > 0x7fffffff or tuple(result[:2]) > tuple(result[2:]):
        raise ValueError("invalid SCIP source range")
    return result


def syntax_classes(tree):
    calls = set()
    nodes = [tree]
    def callee(node):
        while node.get("kind") == "PAREN_EXPR":
            children_ = [c for c in node.get("children", []) if c.get("type") == "Node"]
            if not children_:
                return None
            node = children_[0]
        if node.get("kind") != "PATH_EXPR":
            return None
        path = next((c for c in node.get("children", []) if c.get("kind") == "PATH"), None)
        if not path:
            return None
        segment = next((c for c in reversed(path.get("children", [])) if c.get("kind") == "PATH_SEGMENT"), None)
        return next((c for c in (segment or {}).get("children", []) if c.get("kind") == "NAME_REF"), None)
    while nodes:
        node = nodes.pop()
        children_ = node.get("children", [])
        anchor = None
        if node.get("kind") == "CALL_EXPR":
            function = next((c for c in children_ if c.get("type") == "Node"), None)
            anchor = callee(function) if function else None
        elif node.get("kind") == "METHOD_CALL_EXPR":
            anchor = next((c for c in children_ if c.get("kind") == "NAME_REF"), None)
        if anchor:
            calls.add((anchor["start"][0], anchor["end"][0]))
        nodes.extend(children_)
    result = {}
    nodes = [(tree, False, False)]
    while nodes:
        node, imported, macro = nodes.pop()
        imported |= node.get("kind") in ("USE", "USE_TREE")
        macro |= node.get("kind") in ("MACRO_CALL", "MACRO_RULES", "TOKEN_TREE")
        if node.get("type") == "Token" and node.get("kind") == "IDENT":
            key = (node["start"][1], node["start"][2], node["end"][1], node["end"][2])
            result[key] = "import" if imported else "macro_token" if macro else "direct_call" if (node["start"][0], node["end"][0]) in calls else "value_or_other"
        nodes.extend((child, imported, macro) for child in node.get("children", []))
    return result


def external_method_descriptor(symbol, local_package):
    """Strict rust-analyzer Cargo symbol grammar; no guessed source semantics.

    SCIP Method suffix establishes a callable descriptor, not fine-grained Kind.
    Escaped identifiers are consumed whole so embedded (). is never a method.
    """
    header = re.fullmatch(r"rust-analyzer cargo ([A-Za-z0-9_-]+) ([^\s]+) (.+)", symbol)
    if not header or (header[1], header[2]) == local_package:
        return False
    path = header[3]
    pos = 0
    last = None
    simple = re.compile(r"[A-Za-z0-9_+$-]+")

    def identifier(at):
        if at >= len(path):
            return None
        if path[at] != "`":
            match = simple.match(path, at)
            return match.end() if match else None
        at += 1
        start = at
        while at < len(path):
            if path[at] == "`":
                if at + 1 < len(path) and path[at + 1] == "`":
                    at += 2
                    continue
                return at + 1 if at > start else None
            at += 1
        return None

    while pos < len(path):
        if path[pos] in "[(":
            close = "]" if path[pos] == "[" else ")"
            end = identifier(pos + 1)
            if end is None or end >= len(path) or path[end] != close:
                return False
            pos, last = end + 1, "parameter"
            continue
        end = identifier(pos)
        if end is None or end >= len(path):
            return False
        suffix = path[end]
        if suffix in "/#.:!":
            pos, last = end + 1, suffix
        elif suffix == "(":
            end += 1
            if end < len(path) and path[end] != ")":
                match = simple.match(path, end)
                if not match:
                    return False
                end = match.end()
            if path[end:end + 2] != ").":
                return False
            pos, last = end + 2, "method"
        else:
            return False
    return last == "method"


def labels(index_path, root, syntax_root=None, *, include_external_descriptors=False):
    root = Path(root).resolve()
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    local_prefix = f"rust-analyzer cargo {package['name']} {package['version']} "
    index_path = Path(index_path)
    index = fields(index_path.read_bytes())
    definitions = collections.defaultdict(set)
    kinds = collections.defaultdict(set)
    occurrences = []
    documents = []
    library_documents = set()
    syntax = {}
    for document in children(index, 2):
        relative = string(document, 1)
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("unsafe SCIP document path")
        if integer(document, 6) != 1:
            raise ValueError("SCIP document must explicitly use UTF-8 byte columns")
        documents.append(relative)
        if syntax_root:
            ast = Path(syntax_root) / (relative + ".json")
            if ast.exists():
                syntax[relative] = syntax_classes(json.loads(ast.read_text()))
        def scoped(symbol):
            return relative + "\0" + symbol if symbol.startswith("local ") else symbol
        for symbol in children(document, 3):
            key = scoped(string(symbol, 1))
            kind = integer(symbol, 5)
            kinds[key].add(kind)
            if key.startswith(local_prefix + "lib/"):
                library_documents.add(relative)
        for occurrence in children(document, 2):
            symbol = scoped(string(occurrence, 2))
            location = source_range(occurrence)
            if integer(occurrence, 3) & 1:
                definitions[symbol].add((relative, *location))
            else:
                occurrences.append(dict(path=relative, range=location, symbol=symbol))
    for symbol in children(index, 3):
        kinds[string(symbol, 1)].add(integer(symbol, 5))
    occurrence_symbols = collections.defaultdict(set)
    for occurrence in occurrences:
        occurrence_symbols[(occurrence["path"], tuple(occurrence["range"]))].add(occurrence["symbol"])
    cases = []
    excluded = collections.Counter()
    for occurrence in occurrences:
        kind = kinds.get(occurrence["symbol"], set())
        descriptor_negative = (include_external_descriptors and not kind
            and external_method_descriptor(occurrence["symbol"], (package["name"], package["version"]))
            and not definitions[occurrence["symbol"]]
            and syntax.get(occurrence["path"], {}).get(tuple(occurrence["range"])) == "direct_call")
        if descriptor_negative and len(occurrence_symbols[(occurrence["path"], tuple(occurrence["range"]))]) != 1:
            excluded["external_descriptor_ambiguous_anchor"] += 1
            continue
        if not kind and not descriptor_negative:
            excluded["oracle_symbol_kind_unavailable"] += 1
            continue
        if not kind.issubset({9, 17, 26}):
            excluded["non_callable_symbol_kind"] += 1
            continue
        if not occurrence["path"].startswith("src/"):
            excluded["outside_library_source"] += 1
            continue
        if occurrence["path"] not in library_documents:
            excluded["source_outside_library_compilation_unit"] += 1
            continue
        if occurrence["symbol"].startswith(local_prefix) and not occurrence["symbol"].startswith(local_prefix + "lib/"):
            excluded["target_outside_library_compilation_unit"] += 1
            continue
        targets = sorted(definitions[occurrence["symbol"]])
        if len(targets) > 1:
            excluded["multiple_oracle_definition_anchors"] += 1
            continue
        if targets and not targets[0][0].startswith("src/"):
            excluded["target_outside_library_source"] += 1
            continue
        if not targets and (occurrence["symbol"].startswith(local_prefix) or "\0" in occurrence["symbol"]):
            excluded["missing_local_oracle_definition"] += 1
            continue
        if targets:
            path, line, column, end_line, end_column = targets[0]
            source_lines = (root / path).read_bytes().splitlines(keepends=True)
            offset = sum(map(len, source_lines[:line])) + column
            prefix = b"".join(source_lines)[:offset]
            if not re.search(rb"\bfn\s+(?:r#)?$", prefix[-256:]):
                excluded["nonlexical_or_generated_definition_anchor"] += 1
                continue
        if descriptor_negative:
            occurrence["callable_evidence"] = "SCIP Method descriptor + independent CST direct-call anchor"
            occurrence["negative_scope"] = "external package; no captured local definition expected"
        occurrence["kinds"] = sorted(kind)
        occurrence["syntax_class"] = syntax.get(occurrence["path"], {}).get(tuple(occurrence["range"]), "syntax_unknown")
        occurrence["expected"] = [dict(path=t[0], range=list(t[1:])) for t in targets]
        cases.append(occurrence)
    return dict(contract_version=2 if include_external_descriptors else 1, oracle="rust-analyzer SCIP", scip_sha256=hashlib.sha256(index_path.read_bytes()).hexdigest(), documents=sorted(documents), cases=cases,
                excluded=dict(excluded), ambiguous_definition_symbols=sum(len(v) > 1 for v in definitions.values()), conflicting_kind_symbols=sum(len(v)>1 for v in kinds.values()),
                limitations=["language-server definition correspondence, not runtime dispatch or compiler proof", "multiple-definition/macro anchors excluded explicitly", "only callable references in src/; external targets are negative local-binding cases"])


if __name__ == "__main__":
    import sys
    result = labels(sys.argv[1], sys.argv[2])
    Path(sys.argv[3]).write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(dict(cases=len(result["cases"]), excluded=result["excluded"])))
