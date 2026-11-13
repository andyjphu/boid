#!/usr/bin/env python3
"""Explicit Google sign-in, status, and revocation."""
import contextlib
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from _common import Failure, Parser, SCOPES, config_path, credential_lock, emit, get_creds, main, save_creds


def run():
    parser = Parser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--status", action="store_true")
    mode.add_argument("--revoke", action="store_true")
    parser.add_argument("--port", type=int, default=0, help="Use a fixed loopback port for an SSH tunnel; default: choose a free port")
    parser.add_argument("--no-browser", action="store_true", help="Print the authorization link on stderr instead of opening a browser")
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    token = config_path("token.json")
    if args.status:
        result = {"token_present": token.is_file(), "client_present": config_path("client_secret.json").is_file(),
                  "valid": False, "scopes_granted": [], "expiry": None, "refresh_works": False}
        try:
            creds = get_creds(force_refresh=True)
            result.update(valid=True, scopes_granted=list(creds.granted_scopes or creds.scopes or []),
                          expiry=creds.expiry.isoformat() + "Z" if creds.expiry else None, refresh_works=True)
        except Failure as error:
            result.update(error={"code": error.code, "message": error.message, "hint": error.hint})
            emit(result)
            raise SystemExit(error.code)
        return result
    if args.revoke:
        with credential_lock():
            if not token.exists():
                return {"ok": True, "revoked": False, "token_present": False}
            data = json.loads(token.read_text())
            secret = data.get("refresh_token") or data.get("token")
            if not secret:
                raise Failure(2, "Saved token has no revocable credential.", "Remove the invalid token file and run gauth.")
            req = urllib.request.Request("https://oauth2.googleapis.com/revoke",
                data=urllib.parse.urlencode({"token": secret}).encode(), method="POST")
            try:
                with urllib.request.urlopen(req, timeout=30):
                    pass
            except urllib.error.HTTPError as error:
                if error.code != 400:
                    raise Failure(1, "Google token revocation failed; local token retained.", "Retry when Google is reachable.") from None
                # An already-invalid token cannot be used and can be removed.
                try:
                    if json.loads(error.read()).get("error") != "invalid_token":
                        raise Failure(1, "Google rejected revocation; local token retained.")
                except ValueError:
                    raise Failure(1, "Google rejected revocation; local token retained.") from None
            token.unlink()
        return {"ok": True, "revoked": True, "token_present": False}
    if not 0 <= args.port <= 65535 or args.timeout <= 0:
        raise Failure(4, "Port must be 0–65535 and timeout must be positive.")
    client = config_path("client_secret.json")
    if not client.is_file():
        raise Failure(2, "Google Desktop OAuth client is missing.", f"Place the Desktop client JSON at {client} or set GMCPNOW_CLIENT_SECRET.")
    data = json.loads(client.read_text())
    if "installed" not in data:
        raise Failure(4, "A Desktop OAuth client is required for loopback sign-in.", "Download a Desktop client JSON from Google Cloud; web redirect clients are not interchangeable.")
    from google_auth_oauthlib.flow import InstalledAppFlow
    flow = InstalledAppFlow.from_client_config(data, SCOPES, autogenerate_code_verifier=True)
    original = flow.authorization_url
    def authorization_url(**kwargs):
        url, state = original(**kwargs)
        emit({"auth_url": url, "message": "Open this link in the browser. For SSH, forward the chosen localhost port first."}, sys.stderr)
        return url, state
    flow.authorization_url = authorization_url
    # The library's optional human-readable output must not contaminate stdout.
    with contextlib.redirect_stdout(sys.stderr):
        try:
            creds = flow.run_local_server(host="localhost", bind_addr="127.0.0.1", port=args.port,
                open_browser=not args.no_browser, timeout_seconds=args.timeout,
                authorization_prompt_message="", prompt="consent", access_type="offline")
        except Exception as error:
            raise Failure(2, f"Google sign-in did not finish ({type(error).__name__}).",
                          "Retry gauth; for a remote host use --no-browser --port 8765 with SSH forwarding.") from None
        if not creds.refresh_token or not set(SCOPES) <= set(creds.granted_scopes or creds.scopes or []):
            raise Failure(2, "Google did not grant durable access to all three scopes.", "Run gauth again and grant all requested scopes.")
        with credential_lock():
            save_creds(creds)
    return {"ok": True, "token_path": str(token), "scopes_granted": list(creds.granted_scopes or creds.scopes)}


if __name__ == "__main__":
    main(run)
