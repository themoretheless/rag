#!/usr/bin/env python3
"""Captured byte-copy grouping on real MCP with text, OOXML and PDF fixtures."""
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import zipfile
from smoke_file_imports import StartupClient


def office(kind, compression):
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", compression=compression) as archive:
        if kind == "docx":
            archive.writestr("word/document.xml", "<w:document xmlns:w='w'><w:body><w:p><w:r><w:t>PRIVATE_OFFICE</w:t></w:r></w:p></w:body></w:document>")
        else:
            archive.writestr("xl/workbook.xml", "<workbook xmlns:r='r'><sheets><sheet name='Main' sheetId='1' r:id='rId1'/></sheets></workbook>")
            archive.writestr("xl/_rels/workbook.xml.rels", "<Relationships><Relationship Id='rId1' Target='worksheets/sheet1.xml'/></Relationships>")
            archive.writestr("xl/worksheets/sheet1.xml", "<worksheet><sheetData><row><c t='inlineStr'><is><t>PRIVATE_SHEET</t></is></c></row></sheetData></worksheet>")
    return out.getvalue()


def pdf():
    stream = b"BT /F1 12 Tf 50 750 Td (PRIVATE_PDF document text for capture) Tj ET\n"
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"endstream"]
    output = b"%PDF-1.4\n"
    offsets = [0]
    for index, body in enumerate(objects, 1):
        offsets.append(len(output))
        output += f"{index} 0 obj\n".encode() + body + b"\nendobj\n"
    start = len(output)
    output += f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode()
    output += b"".join(f"{offset:010} 00000 n \n".encode() for offset in offsets[1:])
    output += f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n".encode()
    return output


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-source-byte-copies-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        text = "Привет PRIVATE_BYTES\n"
        fixtures = {
            "alpha/plain.txt": text.encode(), "alpha/copy.txt": text.encode(),
            "alpha/bom.txt": b"\xef\xbb\xbf" + text.encode(),
            "alpha/utf16.txt": b"\xff\xfe" + text.encode("utf-16-le"),
            "alpha/utf16copy.txt": b"\xff\xfe" + text.encode("utf-16-le"),
            "alpha/utf16be.txt": b"\xfe\xff" + text.encode("utf-16-be"),
            "alpha/cyrillic.txt": text.encode("cp1251"), "alpha/cyrilliccopy.txt": text.encode("cp1251"),
            "alpha/doc.docx": office("docx", zipfile.ZIP_STORED),
            "alpha/doccopy.docx": office("docx", zipfile.ZIP_STORED),
            "alpha/doccompressed.docx": office("docx", zipfile.ZIP_DEFLATED),
            "alpha/book.xlsx": office("xlsx", zipfile.ZIP_STORED),
            "alpha/bookcopy.xlsx": office("xlsx", zipfile.ZIP_STORED),
            "alpha/bookcompressed.xlsx": office("xlsx", zipfile.ZIP_DEFLATED),
            "alpha/page.pdf": pdf(), "alpha/pagecopy.pdf": pdf(),
            "beta/foreign.txt": text.encode(),
        }
        for name, raw in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        expected_by_hash = {}
        for name, raw in fixtures.items():
            if name.startswith("alpha/"):
                expected_by_hash.setdefault(hashlib.sha256(raw).hexdigest(), []).append(str(source / name))
        expected = {tuple(sorted(v)) for v in expected_by_hash.values() if len(v) > 1}
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                sync = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not sync.get("errors"), sync
            query = dict(project="alpha", grouping="source_byte_copies", literal_mentions=dict(enabled=False))
            report = mcp.call("duplicates", query)
            observed = {tuple(sorted(m["document"]["path"] for m in g["members"])) for g in report["groups"]}
            assert observed == expected and len(observed) == 6, report
            assert report["source_byte_identity"]["capture_status_counts"] == {"qualified_capture": 16}, report
            for group in report["groups"]:
                assert group["kind"] == "exact_captured_source_bytes" and not group["deletion_safe"], group
                for member in group["members"]:
                    capture = member["document"]["source_byte_capture"]["capture"]
                    assert capture["algorithm"] == "blake3" and len(capture["hash"]) == 64, member
                    assert capture["bytes"] == len(Path(member["document"]["path"]).read_bytes()), member
                    assert member["source_bytes_to_primary"]["captured_source_digest_equal"], member
            assert "PRIVATE_" not in json.dumps(report), report
            assert report == mcp.call("duplicates", query)
            # Decoded text groups must expose raw differences without calling them byte copies.
            decoded = mcp.call("duplicates", dict(project="alpha", literal_mentions=dict(enabled=False)))
            comparisons = [m["source_bytes_to_primary"] for g in decoded["groups"] for m in g["members"]]
            assert any(c["same_decoded_text"] and c["captured_source_digest_equal"] is False for c in comparisons), decoded
            assert any(m["document"]["path"].endswith("doccompressed.docx") for g in decoded["groups"] for m in g["members"]), decoded
            # Stored capture is stable when the live source changes, then replaced on sync.
            changed = source / "alpha/copy.txt"
            changed.write_bytes(b"\xef\xbb\xbf" + text.encode())
            assert report == mcp.call("duplicates", query)
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            updated = mcp.call("duplicates", query)
            groups = [set(m["document"]["path"] for m in g["members"]) for g in updated["groups"]]
            assert {str(changed), str(source / "alpha/bom.txt")} in groups, updated
            assert not any(str(source / "alpha/plain.txt") in g for g in groups), updated
            pdf_member = next(m for g in updated["groups"] for m in g["members"] if m["document"]["path"].endswith("pagecopy.pdf"))
            capture = pdf_member["document"]["source_byte_capture"]["capture"]
            bad_current = dict(source_capture_version=1, source_bytes_hash_algorithm="blake3", source_bytes_hash=capture["hash"], source_bytes_count=capture["bytes"], source_text_hash="old")
            for metadata, reason in [({}, "legacy_or_missing_capture"), (bad_current, "source_text_mismatch")]:
                mcp.call("update_document_meta", dict(document_id=pdf_member["document"]["document_id"], metadata_json=json.dumps(metadata)))
                unavailable = mcp.call("duplicates", query)
                assert unavailable["source_byte_identity"]["capture_status_counts"][reason] == 1 and unavailable["groups_total"] == 5, unavailable
                sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
                assert not sync.get("errors"), sync
                repaired = mcp.call("duplicates", query)
                assert repaired["groups_total"] == 6 and repaired["source_byte_identity"]["capture_status_counts"] == {"qualified_capture": 16}, repaired
            capped = mcp.call("duplicates", dict(query, max_members=1, limit=1, primary_comparison=dict(enabled=False)))
            assert capped["groups_total"] == 6 and capped["truncated"] and capped["groups"][0]["members_truncated"], capped
            assert capped["groups"][0]["members"][0]["source_bytes_to_primary"]["captured_source_digest_equal"], capped
            scoped = mcp.call("duplicates", dict(query, path_prefix=str(source / "alpha/subdirectory")))
            assert scoped["groups_total"] == 0, scoped
            status = mcp.call("index_status", dict(project="alpha"))
            result = dict(status="passed", versions=status["versions"], checks=["actual_raw_sha256_partition_oracle", "UTF8_BOM_UTF16_LE_BE_windows1251", "DOCX_and_XLSX_captured_archive_bytes", "same_text_different_archive_compression", "actual_PDF_extraction", "digest_and_byte_count_evidence", "decoded_text_vs_source_bytes", "project_prefix_isolation", "stored_snapshot_without_live_reads", "BOM_only_sync_replacement", "legacy_capture_upgrade_on_sync", "mismatched_text_capture_repair_on_sync", "member_group_caps_independent_of_text_comparison_budget", "body_free_determinism"], production_database_touched=False, source_code_or_SQL_executed=False, current_filesystem_equality_or_deletion_safety_proven=False, representative_scale_or_full_parity_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result))


if __name__ == "__main__":
    main()
