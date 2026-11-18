# Development operations and recovery

Real payments remain disabled. These tools observe and recover development;
none authorizes production activation or changes native tenant policies.

## Monitoring

The independent Cloudflare worker runs every five minutes. It checks `/health`
(allowing the free host to wake), then token-protected `/health/ready`. Readiness
reads fixed aggregate TypeDB queries and decrypts an external vault canary. Its
credential cannot execute data queries or tenant operations. No business IDs,
credentials or customer payloads are returned. Dependency failures return a
redacted 503. The existing admission middleware still applies.

Separate durable incidents cover availability, overdue jobs (15 minutes),
uncertain runs/invoices and stale reconciliation (one hour). Failed jobs/runs
are cumulative counters: increases alert without deleting history. Two bad
samples open an incident; two healthy samples close it. A D1 outbox records each
message and stable provider idempotency key before delivery. Ambiguous sends
older than 23 hours require operator reconciliation rather than unsafe resend.
The status response exposes pending delivery so a broken mail sender is visible.

Alerts go to dev@jitpomi.com. `/status` has a separate read-only token;
`/check` and `/drill/down|up` require the admin token. Drills are explicitly
labelled, use a separate incident and do not change application health or data.
Retain provider acknowledgements and confirm delivery in the mailbox.

An independent GitHub workflow checks the worker heartbeat and alert outbox.
After 15 minutes without a fresh heartbeat or with undelivered alerts, it sends
an operational warning directly. Scheduled GitHub Actions can be delayed; this
is not a guaranteed five-minute SLA. Its manual drill exercises that separate
sender. Both paths currently share Resend and the destination mailbox: provider
or mailbox outages remain a common dependency.

Provision the worker's D1 schema and secrets before deploying it. Required
worker secrets: OPERATIONS_READ_TOKEN, MONITOR_STATUS_TOKEN,
MONITOR_ADMIN_TOKEN, ALERT_TOKEN. Required variables: BUSINESS_URL, ALERT_FROM,
ALERT_TO. The Business service needs OPERATIONS_ENABLED=true,
OPERATIONS_READ_TOKEN and OPERATIONS_VAULT_CANARY_REF. Seed the canary with
`operations seed-canary` using the explicit private BUSINESS_ENV_FILE.

Configure the watchdog's BUSINESS_MONITOR_URL, BUSINESS_OPERATIONS_ALERT_TO,
BUSINESS_OPERATIONS_ALERT_FROM repository variables and
BUSINESS_MONITOR_STATUS_TOKEN/BUSINESS_OPERATIONS_ALERT_TOKEN secrets. Only set
BUSINESS_MONITOR_ENABLED=true after a successful protected readiness check.
Do not commit values, screenshots of plaintext secrets, or private exports.

## Consistent recovery checkpoint

1. Suspend the development service and all other writers; confirm stopped.
2. With an explicit private BUSINESS_ENV_FILE, run `database_backup export
   business_native_dev_v5 PRIVATE_DIRECTORY`. This captures schema, binary
   data, inventory, records and a canonical complete graph, and refuses a
   checkpoint if facts changed while exporting. Never reset the source.
3. Export the entire ciphertext D1 database using official Wrangler `d1 export
   --remote`. Include all rows, not just currently referenced connections. Export
   the monitor D1 state/outbox too so alert retries retain their identity.
4. Preserve private runtime configuration, the vault master key, deployment
   identifiers, exact source archives and lockfiles in a private bundle directory.
   Include infrastructure configs and this runbook. Document the checkpoint UTC
   time: the recovery point is this checkpoint, not later provider activity.
5. `python3 deploy/recovery/bundle.py pack PRIVATE_DIRECTORY PRIVATE.zip` makes a
   bounded SHA-256 manifested bundle. `recovery encrypt PRIVATE.zip ARCHIVE.enc
   SEPARATE_KEY.txt` creates an authenticated AES-256-GCM envelope and a private
   printable recovery key. Upload only ARCHIVE.enc to the private recovery Drive
   folder. Put SEPARATE_KEY.txt in a password manager separately. Losing the key
   makes the archive unusable; retaining it beside the archive is not escrow.
6. Resume development after verifying a valid checkpoint. Never enable live
   payments as part of recovery, and never replay uncertain provider actions.

## Restore rehearsal / disaster recovery

Download the encrypted archive from Drive into a fresh private directory.
Authenticate/decrypt it with `recovery decrypt ARCHIVE.enc NEW.zip KEY.txt`, then
`bundle.py unpack NEW.zip NEW_DIRECTORY`. Wrong keys, modified ciphertext,
manifest mismatches, traversal and existing destinations must fail closed.

With verified TypeDB TLS and explicit configuration, run `recovery restore
CHECKPOINT_DIRECTORY SOURCE_NAME business_backend_test_UUID`. It refuses an
existing target or source alias, imports schema/data, compares the complete
graph (including relationship roles) and verifies the native runtime schema.
Never point the restored app at the source database.

Import the complete SQL export into a *new* D1 database and bind a separate
secret-store worker with a new access token. Compare all IDs, ciphertext and
creation times. `recovery verify-vault SHADOW_NAME SQL_EXPORT` decrypts every
connection credential from the restored export and rejects wrong tenant
contexts and wrong keys; it never contacts the original secret store.

Start the recovered app with BUSINESS_RECOVERY_MODE=true, the generated shadow
TYPEDB_DB and the shadow vault URL/token. Recovery mode suppresses background
workers and rejects writes, custom write capabilities, webhooks and payment
execution at ingress. Permitted reads still run dog-auth and native TypeDB
RBAC. Check authenticated readiness, unauthenticated denial, cross-tenant
reads, native membership revocation and payment-write denial. Keep source and
shadow graph comparisons and journal counts as evidence. Before returning to
normal operation, reconcile provider state against restored journals without
manually marking invoices paid or blindly retrying uncertain effects.

This archive covers application state and encrypted plugin credentials; it is
not an export of Auth0's password database, financial providers or Gmail. Auth0
and provider control-plane access must remain recoverable independently. A lost
website SESSION_KEY can be rotated, invalidating sessions and requiring fresh
login; preserve/reconfigure the origin secret consistently. Do not claim an
IdP outage or lost external provider account has been recovered by a TypeDB
restore. Retain the completed rehearsal's measured recovery time and exact
checkpoint timestamp rather than promising an unmeasured RPO/RTO.
