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

## Daily encrypted offsite checkpoints

`Business encrypted backups` runs daily at 09:17 UTC, independently of an
operator computer, and supports manual dispatch. It retains each successful
recipient-encrypted GitHub artifact for 30 days. GitHub schedules can be delayed;
a failed run sends an immediate operations email, and the independent monitor
alerts if the last acknowledged checkpoint is over 36 hours old. Existing
backups are never replaced by a failed run. Repository access and artifact
retention remain external dependencies; keep the separately verified Drive
recovery checkpoint and password-manager identity as another recovery path.

The job receives only the development database connection, dedicated read-only
ciphertext/monitor snapshot credentials, the public age recipient and an already
encrypted configuration capsule. It does not receive the age recovery identity,
plaintext vault master key, payment credentials or decrypted runtime configuration.
Build steps receive no backup credentials. Only `business-backup.age` is uploaded;
runner plaintext is removed afterwards. Secrets are never embedded in commands
or logs. Re-seal the configuration capsule after runtime credential/configuration
changes; changing the recipient requires a coordinated new capsule and a tested
recovery identity, never silently rotating away the only usable key.

The [native TypeDB online export](https://typedb.com/docs/maintenance-operation/database-export-import/) is transactionally consistent while source
writes continue. The source cluster performs only the export. Its schema/data are
imported into a generated database in a separate TypeDB 3.15.0 runner container,
pinned by the official multi-platform image digest, before inventory, records and
relationship-role manifests are computed. This keeps restore load away from the
application cluster. The container publishes gRPC only on runner IPv4 loopback;
its disposable default account receives no cloud credentials or runtime keys.
`BUSINESS_RECOVERY_LOCAL_ADDR` is required and rejects remote addresses. The
ciphertext store is immutable and is captured *after* the TypeDB export, ensuring
all committed connection references are included. A second independent graph
restore in that same isolated container verifies the checkpoint. Temporary databases are removed only when this
job created them and verification succeeded. A failed run publishes no successful
checkpoint receipt; runner diagnostics identify the failed stage.
The job checks ciphertext reference completeness but cannot decrypt credentials
without the separately held recovery identity. Operator drills test that stage.
The runner container and its volumes are discarded after the job, including on
failure; existing source databases, offsite artifacts and operator checkpoints
are retained. No restore or failed-run replay is attempted on the application
cluster. An operator using `database_backup export-online` must also start the
explicit loopback verifier; the command has no source-cluster fallback.

The offsite artifact is acknowledged to the monitor only after upload succeeds,
with its SHA-256, checkpoint time and verified-graph result. Old receipts cannot
advance freshness. Artifact retention is 30 days, not indefinite archiving or a
continuous backup guarantee. The daily recovery point can be up to one day old;
alerts at 36 hours expose missed runs instead of claiming a newer recovery point.

For recovery, download the encrypted artifact, use the password-manager age
identity to decrypt it to a private ZIP, and verify/unpack it with `bundle.py`.
Decrypt its nested `configuration.age` with the same identity, then verify/unpack
that configuration ZIP into a separate private directory. This restores runtime
and monitor configuration without ever giving the backup runner the private key.
Restore TypeDB and SQL stores as above, using a new database/store and
BUSINESS_RECOVERY_MODE=true. Do not enable workers or payment writes until the
restored native policies, credentials and provider journals have been verified.

## Closing uncertain automation attempts

An `attention` run is not proof that an external action failed or succeeded.
Cancellation of such a run requires 8–2000 bytes of evidence under the native
`automation_control` policy and an active tenant owner in the same write
transaction. The API accepts `action: cancel`, `id`, and `evidence`.
The original attempt, idempotency key, workflow revision, cursor and error are
retained; an audit entry records actor, time and evidence. Cancellation never
calls a provider, changes an invoice/payment status or asserts successful
completion. A later worker delivery remains a no-op for the cancelled run.

The control-policy update is explicit: pause writers, take a fresh checkpoint,
and use `operations refresh-control-policy DEVELOPMENT_DATABASE GRAPH_CHECKPOINT.json`.
It refuses a stale graph or production database, updates only the canonical
control function in one schema transaction, and checks that the graph remains
identical. There are no startup schema writes or authorization fallbacks.


### Backup revision during a staged migration

Recovery tools must match the active database schema. Set the development repository variable `BUSINESS_BACKUP_RUNTIME_REF` to the full immutable commit deployed on the development service while a newer schema migration is staged on main. Production uses only its separate `BUSINESS_PRODUCTION_BACKUP_RUNTIME_REF`; an unset profile defaults to the workflow commit. Branch names and tags are rejected.

The workflow continues to export the explicitly configured database, independently restore and compare its graph, verify its native schema, capture its ciphertext stores, and encrypt the runtime capsule. It archives the selected source commit alongside the checkpoint. This preserves strict schema verification without requiring an undeployed migration to succeed first. Update the database, runtime capsule and recovery revision together only after the coordinated cutover succeeds. Never point a current schema at older recovery code merely to silence a failed check.
