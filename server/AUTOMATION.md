# Tenant automation

The backend runs durable workflows over tenant-owned connections. Billing is one
installed capability. The engine knows about events, steps, claims and outcomes;
financial rules remain in `billing`, provider contracts in `finance`, and vendor
translation in connector packages. A new trusted connector does not require
changing the execution engine or tenant service.

## What runs today

- Owner-authenticated `/automation` API with the existing JWT and tenant context.
  A request body cannot select another tenant. Owner membership is checked fresh;
  an external action's database claim also checks the workflow owner's role and
  the connection record revision in the claim transaction.
- Encrypted credentials, explicit verification and disconnection. AES-256-GCM
  authenticates tenant, connection ID and immutable connection revision. Credentials
  and ciphertext are excluded from API responses. Generic `/data` cannot read or
  forge automation records, including when the caller owns the tenant.
- Published workflow snapshots, JSON mapping, equality filters, durable waits,
  approvals, scheduled and authenticated webhook triggers, cancellation, blocked
  run resumption, and explicit reconciliation of uncertain writes.
- HTTP, Stripe, Mercury, Resend and billing connectors. Trusted packages implement
  `Connector`; packages can additionally provide `FinancialPlugin` or
  `NotificationPlugin` capabilities. `app::build_with_connectors` installs them.
- HTTP connections use public HTTPS origins, fresh DNS validation and pinned
  resolved addresses. Private/reserved destinations, cross-origin requests,
  redirects, proxy inheritance and automatic HTTP retries are rejected. Bearer
  and API-key authentication are supported. This connector intentionally cannot
  reach local networks; private systems require a separately reviewed connector.

This is an API and worker foundation. A visual editor, OAuth consent/refresh,
user-uploaded connector code, arbitrary scripts, calendar cron expressions and
arbitrary workflow graphs are not implemented. Additional protocols and provider
capabilities require trusted adapters. Connecting a service does not grant
capabilities absent from that service's API or credentials.

## Execution and recovery

Every run is deduplicated by tenant, workflow ID and caller-supplied event key.
Reusing that identity with different input fails. The run retains its workflow,
owner and connection revisions. Workflow versions are saved immutably before
publication; even a failed publication cannot discard the submitted definition.
Updating a workflow requires its current `revision`; stale edits fail. A new
workflow omits `revision`. Inspect a saved `workflow_version` using its revision
UUID. Editing a published workflow affects future runs;
existing runs keep their snapshot. Disabling a workflow stops new runs; cancel
existing runs separately. Disabling a connection blocks subsequent actions;
an already-started request may finish.

A worker persists an attempt and lease before calling an external system. Claims
use compare-and-swap and tenant revision arbitration. A crash, lost commit
acknowledgement or concurrent worker cannot authorize a second non-idempotent
write. Read actions and explicitly idempotent trusted actions can retry at most
three times with the same action key. Generic HTTP writes and mail sends are
at-most-once attempts: any uncertain outcome requires owner review. An
`Idempotency-Key` header alone never makes an unknown API safely retryable.

`resolve` records who verified an external outcome and the evidence/result,
then advances past the action; it never sends it again. Confirm the result in the
external system first. If no successful result can be established, cancel and
review the intended work before creating another run. Restarting a worker never
resets financial consent, issuance, collection or notification claims.

Schedules enqueue a deterministic occurrence before advancing their cursor. If
the process stops between those commits, the next pass finds the same run. Missed
occurrences are retained and processed in bounded passes. A schedule whose event
cannot be queued becomes blocked; inspect it and re-submit its unchanged
configuration to resume after fixing the workflow/owner/connection. Changing its
workflow, input or interval requires a new schedule ID. This prevents an occurrence
identity being silently reused for different work.

The worker discovers due tenants every two seconds and processes up to four
records per tenant per pass, with up to four tenants concurrently. It discovers
at most 1,000 due tenants per pass. Ordinary lists return at most 100 records;
run lists contain metadata rather than all outputs. Use `inspect` for a known ID.
`{"action":"list","kind":"workflow_version","limit":100,"after":null}` returns
metadata and a `next` cursor; pass that cursor as `after` to continue. Pagination
is scoped to the authenticated tenant and record kind. Saved versions remain
accessible even when a publication acknowledgement is lost.
Connections are capped at 100 per tenant. Workflows allow 64 unique steps and 64 KiB
of definition; event input is 64 KiB, each mapped/action result 256 KiB, and stored
outputs per run 1 MiB. References cannot amplify data past the mapping budget.
Large files should remain in their source system and be passed by reference.

## API examples

Send `Authorization: Bearer <JWT>` and `x-tenant-id: <tenant UUID>` to
`POST /automation`. If the deployment uses the trusted gateway, its existing
`x-gateway-secret` requirement also applies. Secrets belong in `secrets`, not
workflow input or public configuration. Never commit real credentials.

Create a connection, then verify it:

```json
{"action":"connect","id":"crm","connector":"http","config":{"origin":"https://api.example.com","verify_path":"/v1/me"},"secrets":{"bearer":"<token>"}}
```

```json
{"action":"verify","id":"crm"}
```

Verification reports its method. HTTP and financial connectors use a read probe;
choose an authenticated HTTP verification path appropriate to the remote system.
Resend uses configuration validation because sending-only keys cannot list domains;
it does not demand broader permissions or send a test message. Configuration
validation is not proof of authentication, delivery or inbox receipt.

Publish a workflow:

```json
{
  "action":"publish",
  "workflow":{
    "id":"welcome","name":"Welcome a customer","enabled":true,
    "steps":[
      {"kind":"transform","id":"request","value":{"path":"/v1/customers","body":{"name":{"$ref":"input/name"}}}},
      {"kind":"approval","id":"review"},
      {"kind":"action","id":"create","connection":"crm","action":"send","input":{"$ref":"steps/request"}},
      {"kind":"delay","id":"wait","seconds":3600},
      {"kind":"transform","id":"result","value":{"$ref":"steps/create/body"}}
    ]
  }
}
```

References are exact JSON objects, using JSON pointers under `input` or
`steps/<earlier-step-ID>`. Customer-supplied data is never evaluated as a template
or code. `condition` takes `left` and `equals`; false ends the run as `filtered`.

```json
{"action":"run","workflow":"welcome","key":"customer-123-created","input":{"name":"Jane"}}
```

```json
{"action":"inspect","kind":"run","id":"<returned run ID>"}
```

`inspect` returns data plus storage status, revision and due time for runs,
workflows and schedules. Connection inspection returns only public connection
metadata. Other commands are `approve`, `cancel`, `resume`, `disconnect` (all take
`id`), and `resolve` (`id`, `evidence`, `output`). `GET /automation` returns the
installed catalog, public connections and bounded lists.

A schedule takes `id`, `workflow`, `every_seconds` (60 minimum), `next` (Unix UTC
seconds), `input` and `enabled`. The server controls its occurrence sequence.

Create an event trigger with
`{"action":"trigger","id":"crm-events","workflow":"welcome","enabled":true}`.
The token is returned once. Deliver JSON to
`POST /automation/events/<tenant UUID>/crm-events` with `x-automation-token` and
`Idempotency-Key` headers. The token is stored only as a digest. Repeating an event
is safe; a different tenant or payload does not inherit its authorization. Rotate or change the target workflow
by creating a new trigger ID and disabling the old one. Event ingress has its own
body/concurrency limits and does not accept a caller-selected execution actor.

Connections are immutable; changing accounts/configuration uses a new connection
ID and a new workflow version. Keep old financial connections available while
invoices still reference them. In-place credential rotation, account-identity
verification for rotation and vault master-key rotation need an explicit migration;
there is no silent replacement of an account behind an existing invoice binding.

## Financial settings

Create and verify `stripe`, `mercury` or `resend` connections with tenant-specific
IDs, then `configure_billing` with `settings`:

```json
{"action":"configure_billing","settings":{"sandbox":true,"default_financial":"my-bank","notification":"my-mail","reminder_days":7,"allowed_test_recipients":["approved@example.invalid"]}}
```

Stripe config is `sandbox` and optional `connected_account`; secrets are `token`
and optional `webhook_secret`. Mercury config is `sandbox`, `destination` (deposit
account UUID), and optional `card_enabled` / `ach_debit_enabled` (both false by
default); its secret is `token`. Resend config is `sandbox`, `from` and optional
`allowed_test_recipients`; its secret is `token`. Generic mail workflows use that
connection's recipient allowlist; billing uses the billing settings' allowlist.

The billing adapter exposes `tick`, `enqueue_due`, `issue`, `reconcile`,
`collect_due` and `notify` and takes its tenant exclusively
from the engine invocation. It calls the existing billing business logic. Provider
credentials are resolved from the tenant's encrypted records for each operation;
there is no deployment-credential fallback in managed mode. A tenant with no
billing settings gets an empty runtime with no payment/mail capabilities.

Dynamic `/tenants/<UUID>/webhooks/<plugin ID>`, `/onboarding` and `/recovery`
resolve the same managed runtime. Optional `ACCOUNT_TENANT_ID` adds root aliases.
`BILLING_ENABLED` installs the billing adapter and portal/ingress; turning it off
blocks new billing adapter actions as well. Other automation connectors remain
available. The old billing tick HTTP endpoint, process timer and per-tenant worker list are
removed. `deploy/automation-wake` only requests `/health` to wake a sleeping host;
it contains no business scheduling or billing credential.

## Cutover of an existing database

No existing development or production database is changed by building this code.
Startup refuses billing without automation and refuses automation without its
schema. Auto-deployment remains off. Do not deploy this version onto an old
billing configuration before completing this cutover.

1. Stop every old application writer and scheduler. Inspect and reconcile existing
   uncertain financial claims. Export schema/data/record inventory using the
   existing `database_backup export <target> <private directory>` tool and run its
   `restore-verify` command. Use a fresh directory with mode 700. Retain the old
   deployment, configuration and backup for review; never reset invoice state.
2. Keep a random 32-byte `AUTOMATION_MASTER_KEY_B64` key in a private secret store,
   base64 encoded, outside the database and repository. Back it up securely;
   ciphertext cannot be recovered without it. The runtime needs its own copy.
3. With private operator credentials in `BUSINESS_ENV_FILE`, set
   `AUTOMATION_WRITERS_STOPPED=true` and `AUTOMATION_BACKUP_DIR=<private directory>`.
   Run `cargo run --locked --bin automation -- install <target>`.
   The tool checks the snapshot before installation and verifies existing records
   afterward. It adds only the automation schema; it never resets a database.
4. For each existing billing tenant set `AUTOMATION_IMPORT_TENANT` and
   `AUTOMATION_IMPORT_OWNER` (an active owner identity UUID). Keep its old tenant
   connection/settings variables available **only in the private operator file**.
   Run `cargo run --locked --bin automation -- import <target>`. Writable financial
   connections retain the exact IDs `stripe` / `mercury`, so customer and invoice
   bindings remain valid. Mail becomes `resend`. Read-only proxy grants are not
   upgraded into financial executors; retain their configuration and recreate
   appropriately scoped HTTP/custom connections separately.
5. Import performs read/configuration verification, records encrypted connections
   and the `billing-cycle` workflow/schedule, and checks original records again.
   It makes no payment or email sends. It changes only automation records and
   tenant concurrency revisions. It refuses conflicting connections/settings;
   inspect a partial import before resuming. Take a fresh verified backup before
   another operator run if automation records have since changed.
6. Set runtime `AUTOMATION_ENABLED=true`, `BILLING_ENABLED=true` if billing is
   needed, and the vault key. Remove old provider/settings arrays from the
   deployed managed runtime; it ignores them. Deploy, verify the tenant APIs and
   due workflow, and configure provider webhooks to the tenant routes. Enable the
   health-only wake worker if the host sleeps. Confirm the old scheduler stays off.

Rolling an old binary back while new writes continue is unsafe. Stop writers and
review the preserved database/connection state before any rollback. The additive
schema itself does not require deleting or rewriting existing business data.

Development deployment evidence: [November 15 cutover](ops/development-automation-cutover-2026-11-15.md).
