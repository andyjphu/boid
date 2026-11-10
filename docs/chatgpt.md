# ChatGPT plan access

Boid supports the documented [Sign in with ChatGPT flow for local and open-source
agents](https://developers.openai.com/siwc/token-sharing-open-source) in its own
agent loop. Eligible requests use the selected account's plan allowance or
credits. API-key requests use separate API billing. Boid never switches billing
routes after an error.

## Connect

An existing API-key server can use **Accounts and billing** in the workspace
header to add ChatGPT accounts. To start a new server without an API key:

```sh
BOID_AUTH_ROUTE=chatgpt boid
```

Keep `BOID_AUTH_ROUTE=chatgpt` in that server's environment for subsequent
launches without an API key. The setting initializes the billing default;
previously saved account selections take precedence. The server loads its
credentials, so restarting only a browser does not change its environment.

Open the workspace using `http://127.0.0.1:<port>`, choose **Accounts and billing**,
then **Continue with ChatGPT**. Finish consent in the new tab. Back in Boid, select
the account and an available model, then **Use for new agents**. Signing in alone
does not change the selected billing route. Models come from that account's
current catalog; Boid preserves the selected model slug.

For a remote server, forward its loopback port before signing in. For example,
`ssh -N -L 7734:127.0.0.1:7734 mm-west-2` lets the browser use
`http://127.0.0.1:7734`. The same tunnel carries `/auth/callback`; keep it open
through sign-in. Use the workspace's existing private token to authenticate to
Boid. Neither the Boid token nor OAuth credentials belong in shared URLs or logs.

Existing agents keep their captured billing route, account, and model after
selection changes and restarts. Legacy agents keep API-key billing. The current
model menu shows the agent's configuration; create a new agent to use a different
account or model. Signing out stops agents using that account and removes its
local tokens. Account identity remains available for reconnecting. If revocation
could not be confirmed, disconnect Boid in ChatGPT settings too.

## Limits and recovery

**Using ChatGPT plan** appears next to plan-agent composers. **Manage usage** opens
[ChatGPT usage settings](https://chatgpt.com/settings/usage). Diagnostics retain
token counts but do not assign API dollar estimates to plan requests.

A usage-limit error pauses further requests for that account. Review usage,
choose **Resume requests** in account settings, then send a message to resume the
agent. Boid does not infer a limit reset time. Network failures retain credentials;
terminal refresh failures require reconnecting the saved account. Provider errors
retain HTTP status, error details and request ID for diagnosis. Failed or
interrupted streams do not count as completed responses.

The plan route sends explicit history with `store:false` and `stream:true` to the
public Responses API. Local tools use the `boid` namespace. API-key access-program
settings are omitted. `/clear` works on either route. `/oai-compact` and its model
tool are unavailable on the plan route because support for the separate
compaction endpoint is not established by this integration's documented route.
No API-key compaction request is made as a fallback.

Credentials are stored under `$BOID_STATE_DIR/chatgpt` (by default
`~/.local/share/boid/chatgpt`), with a private directory and owner-only atomic
credential files. Refreshes are serialized, and a cancelled agent request does
not discard a rotated refresh token. Browser storage contains no OAuth tokens.
Treat the server account and its state directory as trusted, as with Boid's
existing API key and command execution.

## Isolated validation and activation

Use an isolated checkout, port, and state directory for validation. Do not run
`build.sh` from a candidate worktree: it updates the shared `boid` launcher link.
Build with `cargo build` and invoke that worktree's binary directly, for example:

```sh
BOID_AUTH_ROUTE=chatgpt BOID_LISTEN=127.0.0.1:17734 \
  BOID_STATE_DIR=/path/to/private-test-state ./target/debug/boid serve
```

Forward the test port if remote, and perform consent there. Automated tests use
an isolated mock OAuth issuer and Responses server, generated temporary signing
keys, and synthetic credentials. They establish local flow and failure handling;
actual account eligibility and provider interoperability still require consent
and a live inference check. Activating a candidate on the shared server is a
separate deployment step.

References: [sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in),
[profiles and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions),
[models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference),
[preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations),
[errors and recovery](https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery).
