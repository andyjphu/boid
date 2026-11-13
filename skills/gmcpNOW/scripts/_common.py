"""Shared CLI, credentials, Google API access, and document identifiers."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import random
import re
import sys
import tempfile
import time
from urllib.parse import parse_qs, urlparse

SCOPES = ["https://www.googleapis.com/auth/" + scope for scope in
          ("documents", "spreadsheets", "drive.readonly")]
MIMES = {"doc": "application/vnd.google-apps.document", "sheet": "application/vnd.google-apps.spreadsheet"}
PRODUCTION_WARNING = ("PRODUCTION GOOGLE WORKSPACE ACCESS. Reads and edits affect real company documents. "
    "Operate only within the user's task. File creation, file deletion, and editing/restoring/deleting version history "
    "are unavailable. Existing-file edits save a local pre-edit snapshot; prefer additive edits in blue.")


class Failure(Exception):
    def __init__(self, code, message, hint="", **details):
        super().__init__(message)
        self.code, self.message, self.hint, self.details = code, message, hint, details


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise Failure(4, message, "Use --help for command syntax.")

    def print_help(self, file=None):
        emit({"help": self.format_help()})


def emit(value, file=None):
    print(json.dumps(value, ensure_ascii=False, allow_nan=False), file=file or sys.stdout, flush=True)


def main(function):
    try:
        emit({"warning": PRODUCTION_WARNING}, sys.stderr)
        result = function()
        if result is not None:
            emit(result)
    except Failure as error:
        emit({"error": {"code": error.code, "message": error.message, "hint": error.hint, **error.details}}, sys.stderr)
        raise SystemExit(error.code)
    except (ValueError, OSError) as error:
        emit({"error": {"code": 4, "message": str(error), "hint": "Check arguments and local input files."}}, sys.stderr)
        raise SystemExit(4)
    except KeyboardInterrupt:
        emit({"error": {"code": 1, "message": "Interrupted.", "hint": "Check the file before retrying a write."}}, sys.stderr)
        raise SystemExit(1)
    except Exception as error:
        # OAuth/HTTP exceptions can contain credentials or complete request URLs.
        emit({"error": {"code": 1, "message": f"Operation failed ({type(error).__name__}).",
                        "hint": "Check service availability; re-read the file before retrying a write."}}, sys.stderr)
        raise SystemExit(1)


def config_path(name):
    key = "GMCPNOW_CLIENT_SECRET" if name == "client_secret.json" else "GMCPNOW_TOKEN"
    return Path(os.environ.get(key, str(Path.home() / ".config/gmcpNOW" / name))).expanduser()


@contextlib.contextmanager
def credential_lock():
    import fcntl
    token = config_path("token.json")
    token.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(str(token) + ".lock", os.O_CREAT | os.O_RDWR, 0o600)
    with os.fdopen(fd, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def save_creds(creds):
    target = config_path("token.json")
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    data = json.loads(creds.to_json())
    # Persist Google's granted scope set, not just the originally requested set.
    data["scopes"] = list(creds.granted_scopes or creds.scopes or [])
    fd, name = tempfile.mkstemp(prefix=".token-", dir=target.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(data, stream)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, target)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def get_creds(force_refresh=False):
    from google.oauth2.credentials import Credentials
    from google.auth.transport.requests import Request
    from google.auth.exceptions import RefreshError, TransportError
    with credential_lock():
        path = config_path("token.json")
        if not path.is_file():
            raise Failure(2, "Google sign-in is missing.", "Run gauth; do not retry document commands until sign-in succeeds.")
        try:
            data = json.loads(path.read_text())
            granted = set(data.get("scopes", []))
            if not set(SCOPES) <= granted:
                raise Failure(2, "The token does not grant all required scopes.", "Run gauth and grant Docs, Sheets and Drive read access.")
            creds = Credentials.from_authorized_user_info(data)
        except (ValueError, KeyError, TypeError):
            raise Failure(2, "The saved Google token is invalid.", "Run gauth.") from None
        if force_refresh or not creds.valid:
            try:
                creds.refresh(Request())
            except (RefreshError, TransportError):
                raise Failure(2, "Google token refresh failed.", "Check connectivity; if it persists, run gauth.") from None
            if creds.granted_scopes is not None and not set(SCOPES) <= set(creds.granted_scopes):
                raise Failure(2, "Google did not grant all required scopes.", "Run gauth.")
            save_creds(creds)
        os.chmod(path, 0o600)
        return creds


def service(name):
    import httplib2
    from google_auth_httplib2 import AuthorizedHttp
    from googleapiclient.discovery import build
    http = AuthorizedHttp(get_creds(), http=httplib2.Http(timeout=45))
    return build(name, {"docs": "v1", "sheets": "v4", "drive": "v3"}[name],
                 http=http, cache_discovery=False)


def execute(request, *, write=False):
    from googleapiclient.errors import HttpError
    for attempt in range(4):
        try:
            return request.execute(num_retries=0)
        except HttpError as error:
            status = error.resp.status
            try:
                detail = json.loads(error.content).get("error", {})
                reasons = [x.get("reason") for x in detail.get("errors", [])]
                message = detail.get("message", "Google API request failed.")
            except (ValueError, TypeError):
                reasons, message = [], "Google API request failed."
            rate = status == 429 or any(r in ("rateLimitExceeded", "userRateLimitExceeded") for r in reasons)
            retry = rate or (500 <= status < 600 and not write)
            if retry and attempt < 3:
                time.sleep(2 ** attempt + random.random())
                continue
            if status == 401 or "insufficientPermissions" in reasons:
                raise Failure(2, message, "Run gauth to renew access.") from None
            if status in (403, 404):
                raise Failure(3, message, "Check the file's account/access and that Docs, Sheets and Drive APIs are enabled.") from None
            if status in (400, 412):
                raise Failure(4, message, "Check the request; re-read if the document revision changed.") from None
            hint = "Re-read the file before retrying: a write may have been applied." if write else "Retry after the service recovers."
            raise Failure(1, message, hint, http_status=status, write_outcome="unknown" if write else None) from None
        except (OSError, TimeoutError) as error:
            if not write and attempt < 3:
                time.sleep(2 ** attempt + random.random())
                continue
            raise Failure(1, f"Google request transport failed ({type(error).__name__}).",
                          "Re-read before retrying a write; its outcome may be unknown." if write else "Check network access.") from None


def parse_id(value):
    value = value.strip()
    if re.fullmatch(r"[A-Za-z0-9_-]+", value):
        return value
    parsed = urlparse(value)
    if parsed.scheme not in ("http", "https") or parsed.hostname not in ("docs.google.com", "drive.google.com"):
        raise Failure(4, "Expected a Google Docs/Sheets/Drive URL or a bare file ID.")
    match = re.search(r"/(?:d|folders)/([A-Za-z0-9_-]+)(?:/|$)", parsed.path)
    found = match.group(1) if match else parse_qs(parsed.query).get("id", [""])[0]
    if not re.fullmatch(r"[A-Za-z0-9_-]+", found):
        raise Failure(4, "No file ID was found in the URL.")
    return found


def detect_type(value):
    parsed = urlparse(value)
    if parsed.hostname == "docs.google.com":
        if parsed.path.startswith("/document/"):
            return "doc"
        if parsed.path.startswith("/spreadsheets/"):
            return "sheet"
    data = execute(service("drive").files().get(fileId=parse_id(value), fields="id,mimeType", supportsAllDrives=True))
    for kind, mime in MIMES.items():
        if data["mimeType"] == mime:
            return kind
    raise Failure(4, "This file is not a Google Doc or Sheet.", "Use a native Google Docs or Sheets file, not a PDF or Office attachment.")


def file_url(kind, file_id):
    return f"https://docs.google.com/{'document' if kind == 'doc' else 'spreadsheets'}/d/{file_id}/edit"


def positive(value):
    try:
        number = int(value)
        if number > 0:
            return number
    except ValueError:
        pass
    raise argparse.ArgumentTypeError("must be a positive integer")


def read_input(value, *, json_input=False):
    if value == "-":
        text = sys.stdin.read()
    elif json_input and value.lstrip().startswith(("[", "{")):
        text = value
    else:
        text = Path(value).expanduser().read_text()
    return json.loads(text) if json_input else text


def batch_input(value):
    data = read_input(value, json_input=True)
    requests = data.get("requests") if isinstance(data, dict) else data
    if not isinstance(requests, list) or not requests or any(not isinstance(r, dict) or len(r) != 1 for r in requests):
        raise Failure(4, "Requests must be a nonempty JSON array of single-operation objects.")
    return requests


def dry_run(requests):
    return {"dry_run": True, "requests": requests}
