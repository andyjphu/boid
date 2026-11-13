#!/usr/bin/env python3
"""Read a Google Doc or Sheet, with bounded output."""
import json
import re
from urllib.parse import parse_qs, urlparse
from _common import Failure, Parser, detect_type, execute, main, parse_id, positive, service
from _docs import choose_tab, render_tab, tabs


def quote_sheet(title):
    return "'" + title.replace("'", "''") + "'"


def bounded_range(value, sheets, max_rows):
    titles = {sheet["title"] for sheet in sheets}
    if not value:
        return quote_sheet(sheets[0]["title"]) + f"!1:{min(20, max_rows) + 1}", min(20, max_rows)
    if value in titles or (value.startswith("'") and value.endswith("'") and value[1:-1].replace("''", "'") in titles):
        title = value if value.startswith("'") else quote_sheet(value)
        return title + f"!1:{max_rows + 1}", max_rows
    prefix, cells = value.rsplit("!", 1) if "!" in value else (quote_sheet(sheets[0]["title"]), value)
    match = re.fullmatch(r"\$?([A-Za-z]*)\$?(\d*)(?::\$?([A-Za-z]*)\$?(\d*))?", cells)
    if not match or not any(match.groups()):
        raise Failure(4, "Use a sheet title or A1 range such as 'Tab!A1:F50'.", "Named ranges and R1C1 notation are not supported by this bounded reader.")
    col1, row1, col2, row2 = match.groups()
    if ":" not in cells and col1 and row1:
        return prefix + "!" + cells, 1
    start = int(row1 or 1)
    if start < 1 or (row2 and int(row2) < start):
        raise Failure(4, "Invalid A1 row bounds.")
    end = min(int(row2) if row2 else start + max_rows, start + max_rows)
    return f"{prefix}!{col1}{start}:{col2 or col1}{end}", min(max_rows, end - start + 1)


def read_doc(value, args):
    file_id = parse_id(value)
    document = execute(service("docs").documents().get(documentId=file_id, includeTabsContent=True))
    url_tab = parse_qs(urlparse(value).query).get("tab", [None])[0]
    tab = choose_tab(document, args.tab or url_tab)
    raw = tab["documentTab"] if args.format == "json" else render_tab(tab, args.format == "markdown")
    if isinstance(raw, str):
        truncated = len(raw) > args.max_chars
        content = raw[:args.max_chars]
    else:
        # A truncated JSON body is not valid input for indexed edits.
        if len(json.dumps(raw, ensure_ascii=False)) > args.max_chars:
            raise Failure(4, "Raw tab JSON exceeds --max-chars.", "Increase --max-chars; raw JSON is never silently cut.")
        content, truncated = raw, False
    return {"type": "doc", "id": file_id, "title": document.get("title", ""),
        "tabs": [{"id": t["tabProperties"]["tabId"], "title": t["tabProperties"].get("title", "")} for t in tabs(document)],
        "tab": tab["tabProperties"]["tabId"], "revision": document.get("revisionId"), "content": content, "truncated": truncated}


def read_sheet(value, args):
    file_id = parse_id(value)
    api = service("sheets")
    document = execute(api.spreadsheets().get(spreadsheetId=file_id,
        fields="spreadsheetId,properties(title),sheets(properties(sheetId,title,gridProperties))"))
    sheets = [{"title": s["properties"]["title"], "sheetId": s["properties"]["sheetId"],
        "rows": s["properties"].get("gridProperties", {}).get("rowCount", 0),
        "cols": s["properties"].get("gridProperties", {}).get("columnCount", 0)} for s in document.get("sheets", [])]
    if not sheets:
        raise Failure(3, "The spreadsheet has no readable sheets.")
    requested = args.range
    range_sheets = sheets
    if not requested:
        parsed = urlparse(value)
        gid = parse_qs(parsed.fragment).get("gid", parse_qs(parsed.query).get("gid", [None]))[0]
        if gid is not None:
            selected = next((s for s in sheets if str(s["sheetId"]) == gid), None)
            if selected is None:
                raise Failure(3, "The sheet gid in the URL was not found.")
            range_sheets = [selected] + [s for s in sheets if s is not selected]
    range_, limit = bounded_range(requested, range_sheets, args.max_rows)
    result = execute(api.spreadsheets().values().get(spreadsheetId=file_id, range=range_, majorDimension="ROWS",
        valueRenderOption="FORMULA" if args.formulas else "FORMATTED_VALUE"))
    values = result.get("values", [])
    return {"type": "sheet", "id": file_id, "title": document.get("properties", {}).get("title", ""),
        "sheets": sheets, "range": result.get("range", range_), "values": values[:limit], "truncated": len(values) > limit}


def run():
    parser = Parser(description=__doc__)
    parser.add_argument("file")
    parser.add_argument("--format", choices=["markdown", "text", "json"], default="markdown")
    parser.add_argument("--tab")
    parser.add_argument("--max-chars", type=positive, default=50000)
    parser.add_argument("--range")
    parser.add_argument("--formulas", action="store_true")
    parser.add_argument("--max-rows", type=positive, default=500)
    args = parser.parse_args()
    return read_doc(args.file, args) if detect_type(args.file) == "doc" else read_sheet(args.file, args)


if __name__ == "__main__":
    main(run)
