# Browser passkeys

Boid can authorize a browser through SSH, register a passkey, and remember that
browser for 30 days. This authenticates access to the workspace; `/login` still
selects an agent's model-provider connection. Existing shared-token access works.
There is no public first-visitor enrollment and no separate account page.

## First enrollment

Keep the SSH tunnel pointing the Mac's `localhost:7734` at the server's loopback
port 7734. The default browser origin is **http://localhost:7734**, with RP ID
`localhost`. Use that name consistently, not `127.0.0.1`. The existing launcher
and token URL continue to work independently.

On the server, through authenticated SSH, run:

```sh
boid passkey enroll --output "$HOME/.local/share/boid/enrollment-link"
```

The output file must not exist. It is created with mode 0600 and contains the
five-minute, single-use enrollment URL. Transfer its contents through the
existing authenticated SSH connection to the browser opener without printing
it, recording it in a transcript, or placing it in shell history. For example,
a local Python launcher can capture SSH stdout privately and pass the result to
`open -a 'Google Chrome'`. Delete the private output file after use.

Open the link, select **Create passkey**, and complete the browser's verification
prompt. Boid removes the capability from the address bar immediately. Starting
registration consumes it; cancellation or expiration requires a new link.
Successful enrollment signs the browser in. On a later signed-out visit, select
**Sign in with passkey**. No shared token is sent to the browser in this flow.

A passkey can sync between devices through a credential provider. Registration
therefore authorizes a credential, not a uniquely identified physical device.

## Configuration and revocation

`BOID_BROWSER_ORIGIN` sets the single exact origin, including scheme and port.
Only HTTPS DNS origins or HTTP localhost are accepted. It is never inferred from
request headers. Configure it before enrollment; changing an enrolled origin
causes startup to fail rather than silently abandoning saved credentials.
Moving to another origin requires a deliberate credential migration/reset.

The operator can inspect and revoke access over SSH:

```sh
boid passkey list
boid passkey revoke CREDENTIAL_OR_SESSION_ID
```

The list contains IDs and timestamps, not cookies or enrollment capabilities.
Revoking a credential also revokes all its browser sessions. Revocation and
expiry close that browser's active terminal sockets and event streams without
stopping server-owned agents or terminals. Already accepted requests or shell
commands are not undone. Other browsers stay connected. Legacy shared-token
access remains an independent recovery path and is not revoked by these commands.
The CLI uses a direct loopback connection, ignores proxy configuration, and does
not start another server or follow redirects.

Credential metadata and public keys, plus hashes of browser session secrets,
are stored in private, atomically replaced `browser-auth.json` under
`BOID_STATE_DIR`. They survive restarts. Challenges and enrollment capabilities
exist only in memory, expire after five minutes, and can be consumed once.
The WebAuthn library checks the challenge, exact origin, RP, signature, and user
verification. HttpOnly, SameSite=Strict cookies bind the flow and browser session;
HTTPS deployments also use Secure. Loopback HTTP relies on the existing SSH
tunnel. A successful passkey login clears any old shared-token browser cookie.

## Validation

`tests/passkey-browser.cjs` uses an isolated server and Chromium's virtual
authenticator to exercise the actual browser and server exchange. It rejects
replay, altered origin/challenge/signature, and missing user verification; checks
live SSE/WebSocket revocation, credential revocation during a pending assertion,
and token fallback. Set `BOID_TEST_RESTART` to an executable fixture restart
helper to verify credentials and browser sessions across a real restart. Never
point the fixture at production; the test rejects port 7734.

These checks do not register a real personal passkey or complete a physical
Touch ID/security-key prompt. The operator must complete that step after deployment.

References: [webauthn-rs](https://docs.rs/webauthn-rs/0.6.2/webauthn_rs/struct.Webauthn.html),
[Swift server passkey guide, including localhost origin guidance](https://www.swift.org/documentation/server/guides/passkeys.html),
[Chromium virtual-authenticator protocol](https://chromedevtools.github.io/devtools-protocol/tot/WebAuthn/).
