#!/usr/bin/env python3
"""Find Google Docs and Sheets by title or full text."""
from _common import MIMES, Parser, execute, file_url, main, positive, service


def quote(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"


def run():
    parser = Parser(description=__doc__)
    parser.add_argument("--type", choices=["doc", "sheet", "all"], default="all")
    parser.add_argument("--query")
    parser.add_argument("--name")
    parser.add_argument("--limit", type=positive, default=20)
    args = parser.parse_args()
    kinds = list(MIMES) if args.type == "all" else [args.type]
    filters = ["trashed = false", "(" + " or ".join("mimeType = " + quote(MIMES[k]) for k in kinds) + ")"]
    if args.query:
        filters.append("fullText contains " + quote(args.query))
    if args.name:
        filters.append("name contains " + quote(args.name))
    api = service("drive")
    files, token, incomplete = [], None, False
    while len(files) < args.limit:
        result = execute(api.files().list(q=" and ".join(filters), pageSize=min(1000, args.limit - len(files)),
            pageToken=token, orderBy="modifiedTime desc", includeItemsFromAllDrives=True, supportsAllDrives=True,
            fields="nextPageToken,incompleteSearch,files(id,name,mimeType,modifiedTime,owners(displayName,emailAddress),webViewLink)"))
        incomplete |= result.get("incompleteSearch", False)
        for entry in result.get("files", []):
            kind = next(k for k, mime in MIMES.items() if mime == entry["mimeType"])
            files.append({"id": entry["id"], "name": entry["name"], "type": kind,
                "modified": entry.get("modifiedTime"),
                "owner": ", ".join(o.get("emailAddress", o.get("displayName", "")) for o in entry.get("owners", [])) or None,
                "url": entry.get("webViewLink") or file_url(kind, entry["id"])})
        token = result.get("nextPageToken")
        if not token:
            break
    return {"files": files, "has_more": bool(token), "incomplete_search": incomplete}


if __name__ == "__main__":
    main(run)
