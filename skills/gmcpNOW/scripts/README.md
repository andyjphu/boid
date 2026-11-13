# gmcpNOW command reference

**PRODUCTION ACCESS:** use only within the user's task. Reading/listing company
Docs and Sheets may be implicit in a research assignment. No extra confirmation
flag is required. Existing-file edits save a pre-edit snapshot. **File creation,
file deletion, and modification/restoration/deletion of versions are unavailable.**
Do not bypass those limits with other tools. The earlier approval flags and
scratch-file creation proposal are superseded by Andy's final scope.

## Installation and authentication

Python3.10+, Google APIs directly, no MCP server. Run `python3 scripts/install.py`
from the package. On mm-west-2 use `~/.local/bin/python3.12`: the Homebrew Python
installations failed interpreter/ensurepip checks. The installer uses uv when
available, otherwise venv/pip. It installs a versioned package under
`~/.local/share/gmcpNOW/packages`, a skill symlink at `~/.agents/skills/gmcpNOW`,
a venv at `~/.local/share/gmcpNOW/venv`, and wrappers in `~/.local/bin`.
Wrappers: `gauth`, `glist`, `gread`, `gedit`, plus matching `.py` aliases.
Boid discovers the skill in `/skills` and at the next operator-message boundary;
existing tool continuations keep their skill snapshot. No server restart needed.

Use a Google Desktop OAuth client with Docs, Sheets and Drive APIs enabled.
Requested scopes: `documents`, `spreadsheets`, `drive.readonly` under
`https://www.googleapis.com/auth/`. This tool requests no Drive file-write scope
and exposes no Slides API even if the OAuth project offers broader consent.
Client JSON: `~/.config/gmcpNOW/client_secret.json` (override `GMCPNOW_CLIENT_SECRET`).
Token: `~/.config/gmcpNOW/token.json` (override `GMCPNOW_TOKEN`), atomic writes/mode600.
Never store credentials in Git. OAuth consent grants API access, not unrelated tasks.

```sh
gauth                 # free loopback port; opens browser
gauth --status        # no browser; actually checks refresh
gauth --revoke        # disconnect this account; does not touch Drive files
```

For a local browser and remote CLI:

```sh
ssh -L 8765:127.0.0.1:8765 mm-west-2
# In the remote shell:
~/.local/bin/gauth --no-browser --port 8765 --timeout 600
```

Open `auth_url` from stderr locally and keep SSH connected. Only gauth initiates
interactive sign-in. Other commands fail with exit2 on missing/invalid credentials.
A web client with unrelated redirect URIs is not a Desktop-client substitute.

## List and read

```sh
glist [--type doc|sheet|all] [--query TEXT] [--name TEXT] [--limit 20]
gread ID_OR_URL [--format markdown|text|json] [--tab TAB_ID] [--max-chars 50000]
gread ID_OR_URL [--range "'Tab'!A1:F50"] [--formulas] [--max-rows 500]
```

List searches accessible, non-trashed native Docs/Sheets, newest modified first,
including shared-drive items. Query searches full text; name searches titles.
Returns files with id/name/type/modified/owner/url, has_more, incomplete_search.
Shared-drive ownership can be null. No results is not proof a file does not exist.

Docs enumerate nested tabs and default to the first or URL `tab`; --tab wins.
Markdown renders basic headings/emphasis/links/lists/table cells. It is not a
lossless export of every Docs feature. JSON returns the raw selected documentTab
and fails above max-chars rather than returning broken JSON. Text/Markdown output
is capped and marks truncation. Indexed edits use UTF-16 units, including emoji.

Sheets show dimensions and first20 rows of the first or URL gid tab by default.
Explicit ranges accept A1 notation or tab titles, not named ranges/R1C1. Reads
fetch max-rows plus one sentinel row; truncated reports additional nonempty data.
Choose narrow columns for wide sheets. --formulas returns formulas, otherwise
formatted values. Empty cells/trailing empty rows are omitted by Google.

## Edit existing files

```sh
gedit doc append ID_OR_URL --markdown FILE|- [--tab ID] [--color blue|inherit]
gedit doc insert ID_OR_URL --after ANCHOR --markdown FILE|- [--nth N] [--tab ID] [--color blue|inherit]
gedit doc replace ID_OR_URL --find TEXT --with TEXT [--match-case] [--tab ID]
gedit doc batch ID_OR_URL --requests FILE|JSON|-
gedit sheet write ID_OR_URL --range R --values JSON|FILE|- [--raw] [--color blue|inherit]
gedit sheet append ID_OR_URL --range TAB --values JSON|FILE|- [--raw] [--color blue|inherit]
gedit sheet add-tab ID_OR_URL --title T
gedit sheet batch ID_OR_URL --requests FILE|JSON|-
```

Every command accepts --dry-run anywhere. Previews send no mutations. Indexed Doc
previews read current content; raw batches and Sheet previews work offline.
Raw batches accept an array or {"requests":[...]}. They are limited to the Docs/
Sheets batchUpdate APIs for the existing file; no Drive or revision methods.
Adding a Sheet tab changes the existing spreadsheet; it does not create a Drive file.

Markdown insertions support headings, paragraphs, bold, italic, links and lists;
images are rejected. Tables require explicit batches. Insertion adds a paragraph
boundary to protect preceding paragraph styles. Anchors match across text runs
within a paragraph, including table cells, and need --nth when ambiguous.
Replace targets the selected/first tab, never all tabs silently. Ordinary Doc
edits carry requiredRevisionId to reject stale offsets after a concurrent edit.

Blue is the default for inserted Doc text and newly written/appended Sheet cells.
Use --color inherit to leave color unchanged. Prefer blue insertions when useful;
it is not mandatory. Sheet blue formatting is a second API operation on the
returned updated range. If it fails after values succeed, the error explicitly
reports that values were written; do not repeat an append to retry formatting.
Sheet writes use USER_ENTERED unless --raw is supplied; `=` strings are formulas.

Before each real edit, full API-visible Doc content or Sheet grid data is written
privately under `~/.local/share/gmcpNOW/versions/FILE_ID/`, fsynced, hashed and made
read-only (0400). No edit is sent if snapshotting fails. This adds a full read
before Sheet edits and may be expensive for large spreadsheets. Snapshots are
append-only through this tool; no editing/restoring/deleting them is exposed.
The snapshot is the state observed before the edit, not an atomic Sheets lock;
collaborators may edit between read and write. API JSON is not a guaranteed full
fidelity export of every embedded object. Google editor version history remains
available for human recovery and may group changes. The tool does not name or
modify Google revisions and does not promise a distinct named version per API call.

Success includes id/url/changes and before_edit snapshot path/hash. Reads retry
429/5xx up to3 times. Writes retry explicit rate-limit rejection but not ambiguous
5xx/transport failures, which could duplicate appends. Inspect the file before
retrying an uncertain write. No tokens or OAuth exception bodies are printed.

## Checks and references

Run the venv Python with `-m unittest discover -s tests` from the package. Offline
checks cover identifiers, Unicode/style ranges, bounded reads, credentials,
dry runs, snapshots and absent creation/deletion/version-history commands.
Live read checks need consent. Do not create scratch files. Live write testing
requires a user-assigned edit to an existing file; otherwise report it untested.

[Docs tabs](https://developers.google.com/workspace/docs/api/how-tos/tabs),
[Docs requests](https://developers.google.com/workspace/docs/api/reference/rest/v1/documents/request),
[Drive search](https://developers.google.com/workspace/drive/api/guides/search-files),
[Google version history](https://support.google.com/docs/answer/190843),
[revision limits](https://developers.google.com/workspace/drive/api/guides/manage-revisions),
[OAuth loopback](https://googleapis.dev/python/google-auth-oauthlib/latest/reference/google_auth_oauthlib.flow.html).
