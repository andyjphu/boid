import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
import _common as common
from _docs import anchor_index, choose_tab, markdown_plan, render_tab, tabs, utf16
from gread import bounded_range
import gedit


def fixture(text="Hi 😀 anchor, anchor\n"):
    return {"tabProperties": {"tabId": "t.one", "title": "First"}, "documentTab": {
        "body": {"content": [{"startIndex": 1, "endIndex": 1 + utf16(text), "paragraph": {
            "elements": [{"startIndex": 1, "endIndex": 1 + utf16(text), "textRun": {"content": text}}]}}]}}}


class Identifiers(unittest.TestCase):
    def test_urls_and_ids(self):
        for value in ["a_B-123", "https://docs.google.com/document/d/a_B-123/edit?tab=t.one",
                      "https://docs.google.com/spreadsheets/d/a_B-123/edit#gid=0",
                      "https://drive.google.com/open?id=a_B-123"]:
            self.assertEqual(common.parse_id(value), "a_B-123")
        for value in ["https://evil.test/document/d/abc", "https://docs.google.com.evil.test/document/d/abc", "https://docs.google.com/document/", "abc/def", ""]:
            with self.assertRaises(common.Failure):
                common.parse_id(value)

    def test_tabs_and_unicode_anchor(self):
        first, child = fixture(), fixture("Child\n")
        child["tabProperties"]["tabId"] = "t.child"
        first["childTabs"] = [child]
        doc = {"tabs": [first]}
        self.assertEqual(len(tabs(doc)), 2)
        self.assertIs(choose_tab(doc, "t.child"), child)
        self.assertEqual(anchor_index(first, "anchor", 1), 13)
        self.assertEqual(anchor_index(first, "anchor", 2), 21)
        for anchor, nth in [("anchor", None), ("missing", None), ("anchor", 3)]:
            with self.assertRaises(common.Failure) as error:
                anchor_index(first, anchor, nth)
            self.assertEqual(error.exception.code, 3)

    def test_split_runs(self):
        tab = fixture("abc def\n")
        paragraph = tab["documentTab"]["body"]["content"][0]["paragraph"]
        paragraph["elements"] = [{"startIndex": 1, "textRun": {"content": "abc "}},
                                  {"startIndex": 5, "textRun": {"content": "def\n", "textStyle": {"bold": True}}}]
        self.assertEqual(anchor_index(tab, "c de"), 7)


class Markdown(unittest.TestCase):
    def test_indexed_styles_and_lists(self):
        plan = markdown_plan("# Heading\n\n😀 **bold** and *italic* [link](https://example.com)\n\n- One\n- Two\n\n1. Number\n", 11, "t.one")
        text = plan[0]["insertText"]["text"]
        self.assertEqual(text, "Heading\n😀 bold and italic link\nOne\nTwo\nNumber\n")
        bold = next(r["updateTextStyle"] for r in plan if r.get("updateTextStyle", {}).get("textStyle") == {"bold": True})
        self.assertEqual(bold["range"], {"startIndex": 22, "endIndex": 26, "tabId": "t.one"})
        self.assertTrue(any(r.get("updateParagraphStyle", {}).get("paragraphStyle", {}).get("namedStyleType") == "HEADING_1" for r in plan))
        self.assertEqual(sum("createParagraphBullets" in r for r in plan), 3)
        self.assertTrue(any(r.get("updateTextStyle", {}).get("textStyle", {}).get("link") == {"url": "https://example.com"} for r in plan))

    def test_separate_paragraph_keeps_preceding_style(self):
        plan = markdown_plan("## Next", 20, "t.one", separate=True)
        self.assertEqual(plan[0]["insertText"]["text"], "\nNext\n")
        self.assertEqual(plan[1]["updateTextStyle"]["range"]["startIndex"], 21)

    def test_reject_lossy_input(self):
        for value in ["", "![alt](https://example.com/a.png)", "abc\x00def"]:
            with self.assertRaises(common.Failure):
                markdown_plan(value)

    def test_read_styles_table_and_text(self):
        tab = fixture("Bold\n")
        para = tab["documentTab"]["body"]["content"][0]["paragraph"]
        para["paragraphStyle"] = {"namedStyleType": "HEADING_2"}
        para["elements"][0]["textRun"]["textStyle"] = {"bold": True}
        self.assertEqual(render_tab(tab), "## **Bold**\n")
        self.assertEqual(render_tab(tab, False), "Bold\n")
        tab["documentTab"]["body"]["content"].append({"table": {"tableRows": [{"tableCells": [{"content": fixture("cell\n")["documentTab"]["body"]["content"]}]}]}})
        self.assertIn("| cell |\n| --- |", render_tab(tab))


class Bounds(unittest.TestCase):
    def test_sheet_ranges(self):
        sheets = [{"title": "A!B", "rows": 1000}]
        self.assertEqual(bounded_range(None, sheets, 500), ("'A!B'!1:21", 20))
        self.assertEqual(bounded_range("'A!B'!A100:F900", sheets, 20), ("'A!B'!A100:F120", 20))
        self.assertEqual(bounded_range("A:A", sheets, 10), ("'A!B'!A1:A11", 10))
        self.assertEqual(bounded_range("B2", sheets, 10), ("'A!B'!B2", 1))
        with self.assertRaises(common.Failure):
            bounded_range("named_range", sheets, 10)


class Credentials(unittest.TestCase):
    def test_missing_and_scopes(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"GMCPNOW_TOKEN": directory + "/token.json"}):
            with self.assertRaises(common.Failure) as error:
                common.get_creds()
            self.assertEqual(error.exception.code, 2)
            Path(directory, "token.json").write_text(json.dumps({"scopes": [common.SCOPES[0]]}))
            with self.assertRaises(common.Failure) as error:
                common.get_creds()
            self.assertEqual(error.exception.code, 2)

    def test_private_atomic_save_and_refresh(self):
        from google.oauth2.credentials import Credentials
        from datetime import datetime, timedelta
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"GMCPNOW_TOKEN": directory + "/token.json"}):
            creds = Credentials("old", refresh_token="test-refresh", token_uri="https://oauth2.googleapis.com/token",
                                client_id="test", client_secret="test", scopes=common.SCOPES,
                                expiry=datetime.utcnow() - timedelta(hours=1))
            common.save_creds(creds)
            path = Path(directory, "token.json")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            def refresh(obj, request):
                obj.token = "refreshed"
                obj.expiry = datetime.utcnow() + timedelta(hours=1)
            with patch.object(Credentials, "refresh", refresh):
                self.assertEqual(common.get_creds().token, "refreshed")
            self.assertEqual(json.loads(path.read_text())["token"], "refreshed")


class Commands(unittest.TestCase):
    def cli(self, script, *args, input=None):
        with tempfile.TemporaryDirectory() as directory:
            env = dict(os.environ, GMCPNOW_TOKEN=directory + "/missing.json", GMCPNOW_CLIENT_SECRET=directory + "/client.json")
            return subprocess.run([sys.executable, str(SCRIPTS / script), *args], env=env,
                                  input=input, capture_output=True, text=True, timeout=20)

    def test_dry_runs_without_credentials(self):
        cases = [("sheet", "write", "abc", "--range", "A1:A2", "--values", '[[1],["=SUM(A1:A1)"]]'),
                 
                 ("doc", "batch", "abc", "--requests", '[{"insertText":{"location":{"index":1},"text":"x"}}]')]
        for args in cases:
            result = self.cli("gedit.py", *args, "--dry-run", input="# Test\n")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(json.loads(result.stdout)["dry_run"])

    def test_missing_auth_and_invalid_arguments(self):
        result = self.cli("glist.py")
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertEqual(json.loads(result.stderr.splitlines()[-1])["error"]["code"], 2)
        result = self.cli("gread.py", "abc", "--max-rows", "0")
        self.assertEqual(result.returncode, 4)
        self.assertEqual(json.loads(result.stderr.splitlines()[-1])["error"]["code"], 4)
        result = self.cli("gauth.py", "--status")
        self.assertEqual(result.returncode, 2)
        self.assertFalse(json.loads(result.stdout)["valid"])

    def test_doc_edit_uses_revision_and_selected_tab(self):
        api = Mock()
        document = {"documentId": "abc", "revisionId": "rev1", "tabs": [fixture()]}
        replies = [document, {"replies": [{"replaceAllText": {"occurrencesChanged": 2}}]}]
        with patch.object(gedit, "service", return_value=api), patch.object(gedit, "execute", side_effect=replies), patch.object(gedit, "snapshot", return_value={"path":"fixture"}), \
             patch.object(sys, "argv", ["gedit", "doc", "replace", "abc", "--find", "anchor", "--with", "new"]):
            result = gedit.run()
        body = api.documents.return_value.batchUpdate.call_args.kwargs["body"]
        self.assertEqual(body["writeControl"], {"requiredRevisionId": "rev1"})
        self.assertEqual(body["requests"][0]["replaceAllText"]["tabsCriteria"], {"tabIds": ["t.one"]})
        self.assertEqual(result["changes"]["occurrences_changed"], 2)

    def test_ambiguous_write_is_not_replayed(self):
        from googleapiclient.errors import HttpError
        import httplib2
        request = Mock()
        request.execute.side_effect = HttpError(httplib2.Response({"status": "503"}), b'{"error":{"message":"Unavailable"}}')
        with self.assertRaises(common.Failure) as error, patch.object(common.time, "sleep"):
            common.execute(request, write=True)
        self.assertEqual(request.execute.call_count, 1)
        self.assertEqual(error.exception.details["write_outcome"], "unknown")
        request.reset_mock()
        with self.assertRaises(common.Failure), patch.object(common.time, "sleep"):
            common.execute(request)
        self.assertEqual(request.execute.call_count, 4)

    def test_creation_deletion_and_versions_absent(self):
        for kind in ("doc", "sheet"):
            for command in ("create", "delete", "restore", "delete-version", "versions"):
                result = self.cli("gedit.py", kind, command, "abc", "--dry-run")
                self.assertEqual(result.returncode, 4)
                self.assertIn("PRODUCTION", result.stderr)

    def test_snapshot_before_write_and_failure_stops_write(self):
        document = {"documentId": "abc", "revisionId": "rev1", "tabs": [fixture()]}
        api = Mock()
        with patch.object(gedit, "service", return_value=api), patch.object(gedit, "execute", return_value=document) as call, \
             patch.object(gedit, "snapshot", side_effect=common.Failure(1,"Snapshot failed")), \
             patch.object(sys, "argv", ["gedit", "doc", "replace", "abc", "--find", "anchor", "--with", "new"]):
            with self.assertRaises(common.Failure):gedit.run()
        self.assertEqual(call.call_count, 1)
        api.documents.return_value.batchUpdate.assert_not_called()

    def test_snapshots_and_blue(self):
        from _versions import snapshot, blue_cells
        with tempfile.TemporaryDirectory() as directory, patch.object(Path, "home", return_value=Path(directory)):
            first = snapshot("doc", "abc", {"revisionId":"one", "text":"old"})
            second = snapshot("doc", "abc", {"revisionId":"two", "text":"new"})
            self.assertNotEqual(first["path"], second["path"])
            self.assertEqual(Path(first["path"]).stat().st_mode & 0o777, 0o400)
            self.assertEqual(json.loads(Path(first["path"]).read_text())["document"]["text"], "old")
        plan = markdown_plan("new")
        self.assertTrue(any("foregroundColor" in r.get("updateTextStyle", {}).get("textStyle", {}) for r in plan))
        plan = markdown_plan("new", color="inherit")
        self.assertFalse(any("foregroundColor" in r.get("updateTextStyle", {}).get("textStyle", {}) for r in plan))
        request = blue_cells("'Data tab'!A2:C3", {"sheets":[{"properties":{"title":"Data tab","sheetId":9}}]})
        self.assertEqual(request["repeatCell"]["range"], {"sheetId":9,"startRowIndex":1,"endRowIndex":3,"startColumnIndex":0,"endColumnIndex":3})


if __name__ == "__main__":
    unittest.main()
