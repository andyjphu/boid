"""Private pre-edit snapshots. No restore, overwrite, or delete operations."""
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import uuid
from _common import Failure


def snapshot(kind, file_id, document):
    root = Path.home() / ".local/share/gmcpNOW/versions" / file_id
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    path = root / (stamp + "-" + uuid.uuid4().hex[:8] + ".json")
    data = json.dumps({"type": kind, "id": file_id, "captured_at": stamp,
                       "document": document}, ensure_ascii=False, allow_nan=False).encode()
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        path.chmod(0o400)
        directory = os.open(root, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    except OSError:
        raise Failure(1, "Could not preserve the pre-edit snapshot. No edit was sent.", "Repair local snapshot storage before editing.") from None
    return {"path": str(path), "sha256": hashlib.sha256(data).hexdigest(),
            "google_revision": document.get("revisionId"), "kind": "local_pre_edit_snapshot"}


def blue_cells(a1, document):
    title, cells = a1.rsplit("!", 1)
    title = title[1:-1].replace("''", "'") if title.startswith("'") and title.endswith("'") else title
    sheet = next((s for s in document.get("sheets", []) if s["properties"]["title"] == title), None)
    match = re.fullmatch(r"([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?", cells)
    if sheet is None or match is None:
        raise Failure(1, "Values were written but the returned range could not be colored.", "Do not repeat an append; inspect the returned range.")
    first_col, first_row, last_col, last_row = match.groups()
    def column(value):
        result = 0
        for char in value:
            result = result * 26 + ord(char) - ord("A") + 1
        return result
    return {"repeatCell": {"range": {"sheetId": sheet["properties"]["sheetId"],
        "startRowIndex": int(first_row) - 1, "endRowIndex": int(last_row or first_row),
        "startColumnIndex": column(first_col) - 1, "endColumnIndex": column(last_col or first_col)},
        "cell": {"userEnteredFormat": {"textFormat": {"foregroundColor": {"red": 0, "green": .2, "blue": .8}}}},
        "fields": "userEnteredFormat.textFormat.foregroundColor"}}
