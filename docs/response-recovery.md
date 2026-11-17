# Model response recovery

Boid admits local function calls only after validating the completed model
response. A failed generation can therefore be retried before any of its local
tools execute. Retrying can repeat provider inference and its cost; it does not
provide exactly-once execution for hosted provider tools.

`src/api.rs` uses one budget across HTTP, transport, stream and response-assembly
failures: four HTTP attempts and at most 30 minutes of scheduled backoff. This is
not a total request timeout. The provider’s multi-minute overload waits are
accepted, shown as a cancellable countdown, and followed by an automatic retry. Retry-After advice, including nested stream error
headers, is respected; advice beyond the remaining budget stops recovery rather
than being shortened. Authentication, permission, quota, context-limit and
content-filter errors are terminal. Unknown or hosted tool activity blocks stream
replay. Credentials, model settings and encoded input remain fixed across the
attempts, apart from refreshing the same account's token when required.

Completed items are reconciled by stable identity. Stream order can repair a
shortened or reordered final array only when done events independently cover a
contiguous output sequence and every final item is accounted for. Partial
ambiguous sequences, conflicting identities, duplicate calls and malformed
function-call fields fail validation. Observed output indices and item IDs must
be represented in the resolved response. Final encrypted reasoning is retained.

Before retry backoff, and after a rejected final attempt, Session and Hub clear
partial text that would otherwise enter recovered model history. Stream-entry
maps reset so replacement text cannot concatenate onto rejected output. The
visible failed entries remain in the transcript with an interruption/rejection
notice. Only validated completion publishes its completion event. Cancel and
Stop can interrupt backoff; no background retry survives cancellation.

Tests use loopback HTTP streams and isolated session storage. They cover parser
reconciliation, identical-request recovery, terminal errors, retry budgets,
Retry-After, cancellation, model-history isolation and local tool effects. They
do not establish the frequency or exact upstream payload of production errors.
