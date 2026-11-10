# ChatGPT plan access

Boid supports the documented [Sign in with ChatGPT flow for local and open-source
agents](https://developers.openai.com/siwc/token-sharing-open-source) in its own
agent loop. Eligible requests use the selected account's plan allowance or
credits. API-key requests use separate API billing. Boid never switches billing
routes after an error.

## Connect

Type `/login` in an agent composer and press Enter. The inline chooser offers
**API key** or **ChatGPT subscription**. With no agents yet, choose **+agent**,
then **/login** in the new-agent setup. There is no separate account page.

Boid can start without model credentials. Choose **API key** to use the server's
configured key or enter one in the password field. A key entered here is stored
privately on the server, never in model messages or browser storage. API-key
validity is checked by the provider when an agent makes its first request.

Choose **ChatGPT subscription**, then **Continue with ChatGPT** and finish consent
in the new tab. Return to `/login`, select an account,
then **Use subscription**. Signing in alone does not change the selected route.
Use `/model` to choose from that account's available models. Login keeps the
agent's current model when available, otherwise uses the first listed model.
**Escape** returns to the composer without interrupting the agent.

Selections in an existing pane apply to that agent. Selections in new-agent setup
set the default for future agents. Other agents keep their connections. Wait for
a busy agent to finish or interrupt it before changing its connection or model.
`BOID_AUTH_ROUTE` can initialize the default on a new state directory; saved selections take precedence.

Open Boid through `http://127.0.0.1:<port>` for ChatGPT's loopback callback.

For a remote server, forward its loopback port before signing in. For example,
`ssh -N -L 7734:127.0.0.1:7734 mm-west-2` lets the browser use
`http://127.0.0.1:7734`. The same tunnel carries `/auth/callback`; keep it open
through sign-in. Use the workspace's existing private token to authenticate to
Boid. Neither the Boid token nor OAuth credentials belong in shared URLs or logs.

Each agent keeps its selected billing route, account, model, and history across
restarts. Legacy agents keep API-key billing until `/login` changes that pane.
Signing out stops agents using that account and removes its
local tokens. Account identity remains available for reconnecting. If revocation
could not be confirmed, disconnect Boid in ChatGPT settings too.

## Limits and recovery

**Using ChatGPT plan** appears next to plan-agent composers. Limits and credits
are available in [ChatGPT usage settings](https://chatgpt.com/settings/usage). Diagnostics retain
token counts but do not assign API dollar estimates to plan requests.

A usage-limit error pauses further requests for that account. Review usage,
choose **Resume requests** under `/login` → **ChatGPT subscription**, then send a message to resume the
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
not discard a rotated refresh token. Browser storage contains no API keys or OAuth tokens.
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
