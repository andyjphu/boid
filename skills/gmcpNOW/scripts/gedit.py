#!/usr/bin/env python3
"""Edit existing Google Docs and Sheets. --dry-run sends no mutations."""
import sys
from urllib.parse import parse_qs, urlparse
from _common import Failure, Parser, batch_input, dry_run, execute, file_url, main, parse_id, positive, read_input, service
from _docs import anchor_index, choose_tab, end_index, markdown_plan
from _versions import snapshot, blue_cells


def parser():
    root = Parser(description=__doc__)
    kinds = root.add_subparsers(dest="kind", required=True)
    for kind in ("doc", "sheet"):
        group = kinds.add_parser(kind)
        commands = group.add_subparsers(dest="command", required=True)
        if kind == "doc":
            for name in ("append", "insert"):
                command = commands.add_parser(name)
                command.add_argument("file")
                command.add_argument("--markdown", required=True)
                command.add_argument("--tab")
                command.add_argument("--color", choices=["blue", "inherit"], default="blue")
                if name == "insert":
                    command.add_argument("--after", required=True)
                    command.add_argument("--nth", type=positive)
            replace = commands.add_parser("replace")
            replace.add_argument("file")
            replace.add_argument("--find", required=True)
            replace.add_argument("--with", dest="replacement", required=True)
            replace.add_argument("--match-case", action="store_true")
            replace.add_argument("--tab")
        else:
            for name in ("write", "append"):
                command = commands.add_parser(name)
                command.add_argument("file")
                command.add_argument("--range", required=True)
                command.add_argument("--values", required=True)
                command.add_argument("--raw", action="store_true", help="Store literal text instead of interpreting formulas")
                command.add_argument("--color", choices=["blue", "inherit"], default="blue")
            add = commands.add_parser("add-tab")
            add.add_argument("file")
            add.add_argument("--title", required=True)
        batch = commands.add_parser("batch")
        batch.add_argument("file")
        batch.add_argument("--requests", required=True)
    return root


def operation(method, **kwargs):
    return {"method": method, **kwargs}


def success(kind, file_id, changes):
    return {"ok": True, "id": file_id, "url": file_url(kind, file_id), "changes": changes}


def edit_doc(args):
    file_id = parse_id(args.file)
    if args.command == "batch":
        requests = batch_input(args.requests)
        if args.dry_run:
            return dry_run([operation("docs.documents.batchUpdate", documentId=file_id, body={"requests": requests})])
    api = service("docs").documents()
    document = execute(api.get(documentId=file_id, includeTabsContent=True))
    if args.command != "batch":
        url_tab = parse_qs(urlparse(args.file).query).get("tab", [None])[0]
        tab = choose_tab(document, args.tab or url_tab)
        tab_id = tab["tabProperties"]["tabId"]
        if args.command in ("append", "insert"):
            index = end_index(tab) if args.command == "append" else anchor_index(tab, args.after, args.nth)
            requests = markdown_plan(read_input(args.markdown), index, tab_id, separate=index > 1, color=args.color)
        else:
            if not args.find:
                raise Failure(4, "--find must not be empty.")
            replace = {"containsText": {"text": args.find, "matchCase": args.match_case}, "replaceText": args.replacement}
            # Match the tab that gread defaults to, not every tab silently.
            if tab_id:
                replace["tabsCriteria"] = {"tabIds": [tab_id]}
            requests = [{"replaceAllText": replace}]
    body = {"requests": requests}
    if document.get("revisionId"):
        body["writeControl"] = {"requiredRevisionId": document["revisionId"]}
    if args.dry_run:
        return dry_run([operation("docs.documents.batchUpdate", documentId=file_id, body=body)])
    saved = snapshot("doc", file_id, document)
    result = execute(api.batchUpdate(documentId=file_id, body=body), write=True)
    changes = {"requests_applied": len(requests), "replies": result.get("replies", []), "before_edit": saved}
    if args.command == "replace":
        changes["occurrences_changed"] = sum(reply.get("replaceAllText", {}).get("occurrencesChanged", 0) for reply in result.get("replies", []))
    return success("doc", file_id, changes)


def edit_sheet(args):
    file_id = parse_id(args.file)
    if args.command in ("write", "append"):
        kwargs = {"spreadsheetId": file_id, "range": args.range}
        method = {"write": "update", "append": "append"}[args.command]
        values = read_input(args.values, json_input=True)
        if not isinstance(values, list) or not values or any(not isinstance(row, list) for row in values):
            raise Failure(4, "--values must contain a nonempty JSON array of rows.")
        if any(cell is not None and (not isinstance(cell, (str, int, float, bool))) for row in values for cell in row):
            raise Failure(4, "Cells must be strings, numbers, booleans or null.")
        kwargs.update(valueInputOption="RAW" if args.raw else "USER_ENTERED", body={"values": values, "majorDimension": "ROWS"})
        if method == "append":
            kwargs["insertDataOption"] = "INSERT_ROWS"
        if args.dry_run:
            planned = [operation("sheets.spreadsheets.values." + method, **kwargs)]
            if args.color == "blue":
                planned.append(operation("sheets.spreadsheets.batchUpdate", spreadsheetId=file_id,
                    after_values_write="Set the returned updatedRange text color to blue (0, 0.2, 0.8)."))
            return dry_run(planned)
        api = service("sheets").spreadsheets()
        previous = execute(api.get(spreadsheetId=file_id, includeGridData=True))
        saved = snapshot("sheet", file_id, previous)
        result = execute(getattr(api.values(), method)(**kwargs), write=True)
        if args.color == "blue":
            updated = result.get("updatedRange") or result.get("updates", {}).get("updatedRange")
            if updated:
                try:
                    execute(api.batchUpdate(spreadsheetId=file_id, body={"requests": [blue_cells(updated, previous)]}), write=True)
                except Failure as error:
                    error.details.update(values_written=True, before_edit=saved)
                    error.hint += " Values were written; do not repeat an append merely to retry color formatting."
                    raise
    else:
        requests = [{"addSheet": {"properties": {"title": args.title}}}] if args.command == "add-tab" else batch_input(args.requests)
        if args.dry_run:
            return dry_run([operation("sheets.spreadsheets.batchUpdate", spreadsheetId=file_id, body={"requests": requests})])
        api = service("sheets").spreadsheets()
        previous = execute(api.get(spreadsheetId=file_id, includeGridData=True))
        saved = snapshot("sheet", file_id, previous)
        result = execute(api.batchUpdate(spreadsheetId=file_id, body={"requests": requests}), write=True)
    return success("sheet", file_id, {**result, "before_edit": saved})


def run():
    # Accept --dry-run before or after any subcommand.
    argv = sys.argv[1:]
    dry = "--dry-run" in argv
    args = parser().parse_args([a for a in argv if a != "--dry-run"])
    args.dry_run = dry
    return edit_doc(args) if args.kind == "doc" else edit_sheet(args)


if __name__ == "__main__":
    main(run)
