# Credentials and pane assignment

The lock view lists the shared named credential registry alphabetically. A pane
has one credential for all its agent tabs. Switching the visible tab does not
change authentication. Moving an agent into a pane adopts that pane's credential
for the next request. Moving an individual tab to another screen creates a
singleton pane with the source credential.

An already-started model request keeps its API key or subscription account.
API-key retries retain the same key value even after a rekey or deletion.
Subscription retries may refresh the same account while it exists. After local
deletion they can use only the last issued bearer, which may expire or be rejected
by the provider. There is no account or API-key fallback. Removing a subscription
here removes local access; it does not revoke the provider authorization.

## Authenticated local API

These routes use the same Boid bearer token, browser cookie, or authorized device
as the rest of `/api`. Agents with existing server access can use them directly.
Registry responses and events contain metadata only. Keys appear only in the
authenticated write request and the server's owner-only credential file.

| Method and route | JSON input | Result |
| --- | --- | --- |
| `GET /api/credentials` | None | Registry |
| `POST /api/credentials` | `{"type":"api_key","name":"Research","key":"<key>"}` | `{id, registry}` |
| `PATCH /api/credentials/:id` | `{name?, key?}` | `{registry}`; key edits apply only to API credentials |
| `DELETE /api/credentials/:id` | None | `{registry}`; affected panes become unassigned |
| `POST /api/credentials/sign-in` | `{origin, name?, credential_id?, consent?}` | `{start_url}` for existing OAuth flow; omit ID to add |
| `GET /api/pane-groups` | None | `{revision, groups}` |
| `PATCH /api/pane-groups/:id` | `{credential_id: string or null, revision}` | Current group catalog |
| `POST /api/pane-groups/move` | `{pane, target_group_id: string or null, before?, revision}` | Current group catalog; null target creates a singleton |
| `POST /api/sessions?screen=N&group_id=ID` | Existing create fields | Session already joined to the group before any queued task starts |

Registry rows contain `id`, `type` (`api_key` or `chatgpt`), `name`, `connected`,
`paused`, and `plan_enabled`. Registry metadata includes `revision` and
`default_credential_id`. Credential IDs are random UUIDs, never secret material. An omitted or blank new API-key name receives the first unused `openai-api-key-N` name under the credential lock.
Creating a row does not implicitly assign it or replace the default.

Group rows contain `id`, `screen`, ordered `tabs` (`agent:ID` or `terminal:ID`),
`credential_id`, and `credential_type`. A stale revision returns HTTP 409 with `code: "pane_groups_changed"`;
fetch the current catalog before retrying. The browser owns active-tab selection
and outer geometry. The server owns membership, tab order, and credential choice.
The existing `/api/screens/move` route moves only its specified member tab.

SSE broadcasts `credentials` with `registry` and `pane_groups` with
`pane_groups`. The initial snapshot and session/terminal catalog events include
the current `pane_groups` catalog. No event includes credential secrets.

## Storage and migration

`chatgpt/accounts.json` remains an owner-only file in the private state directory.
It holds named API keys and the existing OAuth account records. The first upgrade
imports the prior singleton key and environment key, deduplicating equal values.
The prior effective API key remains the binding for legacy API sessions; existing
subscription IDs stay stable. A persistent migration marker prevents a deleted
environment credential from reappearing at restart.

`screens.json` now also stores groups and their credential IDs. Existing singleton
pane keys become group IDs, preserving browser layout keys. No browser-local
grouping overrides server membership. Missing/deleted credential references are
cleared during startup, including recovery after interruption between credential
deletion and group-metadata saving. Session transcripts and model context contain
only credential identifiers and route metadata, never keys or OAuth tokens.

Empty agents can exist without credentials and make no provider request. Their
first task requires an assigned credential. Model choice is retained across
assignment; CRUD and assignment do not invoke a paid provider validation request.
