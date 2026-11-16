# Business backend

One DogRS application, one canonical TypeDB model, one executable: `server`.

Human first. Simple first. Local first.

## Structure and responsibilities

- `src/app.rs` composes the same application used by the executable and integration tests.
- `src/auth/` uses dog-auth/dog-auth-local for credentials, JWT verification and persisted revocation. People are independent of authentication. Provider plugins own credentials and account lifecycle outside TypeDB; the graph holds issuer/subject identity links and enforces tenant access through schema policies and TypeQL functions. See [identity and credential migration](docs/identity-credentials.md).
- `src/services/data/` exposes only custom `read` and `write`. The frontend sends complete TypeQL. Hooks validate the transport envelope and bind freshly verified scope; dog-typedb's schema-policy executor enforces the model. There is no per-query registry.
- `src/schema.tql`, `src/functions.tql`, `src/schema-policy.tql` and `src/billing.tql` define identities, membership, ownership, grants, reviews and domain rules. Security functions and annotations are not replaced with Rust domain switches.
- Provider resources have `_service`, `_shared`, `_hooks` and `_schema` modules; common provider hooks live in `hooks/`, credentials in `auth-<provider>/`, and HTTP in adapters. Providers do not query TypeDB.
- `src/database.rs` and the `database` CLI handle explicit provisioning. Startup verifies the canonical model and never migrates, seeds or wipes data.
- Tests live under `tests/`; layout tests check that every Rust source and test file belongs to an active Cargo target.

The backend exposes authentication, protected data queries and provider proxies.
An opt-in server-owned billing worker now executes the canonical billing graph;
see **Billing execution** below. Provider proxies remain transport-only. Invitations and recovery use the canonical identity model;
see **Sandbox invitations and password resets** below.

## Provisioning and launch

Use a private environment file outside Git (permissions 0600). See `.env.example`.
Runtime requires non-admin TypeDB credentials, TLS, an existing canonical database
and `AUTH_JWT_SECRET` of at least 32 nonblank bytes.

Use separate operator credentials to provision a **new** target such as
`business_native_dev` when an existing `business_dev` still contains the old model:

```sh
BUSINESS_ENV_FILE=/private/admin.env cargo run --locked --bin database -- create
BUSINESS_ENV_FILE=/private/admin.env cargo run --locked --bin database -- verify
BUSINESS_ENV_FILE=/private/admin.env cargo run --locked --bin database -- bootstrap
BUSINESS_ENV_FILE=/private/runtime.env cargo run --locked --bin server
```

Both environment files must explicitly select the intended `TYPEDB_DB`.
Bootstrap additionally requires `BUSINESS_OWNER_EMAIL`, `BUSINESS_OWNER_NAME`,
`BUSINESS_TENANT_NAME` and `BUSINESS_OWNER_PASSWORD` (12–72 bytes). It works
only on an empty identity registry and uses a unique bootstrap record to arbitrate
concurrent attempts. It prints tenant/identity IDs, never credentials.
Create refuses an existing target; unknown commands fail before connecting.
`list` is read-only; `create-user` requires operator-supplied
`BUSINESS_NEW_USER` and `BUSINESS_NEW_PASSWORD` and never replaces an account.

There is no destructive reset or automatic migration from the old model. Preserve
existing databases and reconcile any provider records before an independently
approved cutover. The Render blueprint targets `business_native_dev`, disables
automatic deployment and keeps credentials external. Changing that file does not
provision or deploy anything.
Any previously deployed billing scheduler must be retired as an explicit
deployment action; removing its source does not disable a remote cron job.

## HTTP contract

Use `Authorization: Bearer <token>` and `x-tenant-id` on protected calls.
Local sign-in is POST `/authentication` with strategy `local`, email and password.
For an email-only portal, strategy `account` privately discovers an active tenant
membership and verifies the same password before returning its tenant ID. No
company or identity information is returned for failed sign-ins. Explicit tenant
sign-in and fresh membership checks on protected calls remain unchanged.
DELETE `/authentication` revokes the supplied token durably.

POST `/data` with `x-service-method: read` or `write` and only
`{"query":"complete TypeQL"}`. The response is the DogRS
`ok.answers` envelope, never a raw query echo. Credentials/revocation fields are
hidden by the model. The publicly served `/queries/data/*.tql` files are examples
for the browser, not an authorization registry; `/data-client.js` sends queries.

`/health` is public. API execution has a per-process active budget
(`BUSINESS_ACTIVE_REQUESTS`, 1–32), 32 waiting slots and a five-second admission
deadline. Overload returns 503 without starting the operation.
If an additional trusted gateway is used, configure `BUSINESS_GATEWAY_SECRET`
(at least 32 bytes) and supply `x-gateway-secret` on API requests. This is
additional protection, never a replacement for tenant authentication.

## Verification

```sh
cargo build --locked --all-targets
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
node --test tests/frontend/data-client.test.mjs ../deploy/automation-wake/worker.test.mjs
BUSINESS_ENV_FILE=/private/dev.env BUSINESS_RUNTIME_ENV_FILE=/private/runtime.env cargo test --locked --test backend -- --ignored --test-threads=1
```

The real integration suite uses existing development-cluster credentials and
generated `business_backend_test_<UUID>` databases only. It checks RBAC
decisions, frontend queries, hidden data, rollback, concurrency, credential rotation
and membership expiry. The executable test provisions and bootstraps a disposable
target using the actual CLI, starts the actual server with non-admin credentials,
then checks gateway enforcement, owner login, data writes/reads and logout.
Disposable targets are cleaned up; configured databases are never deleted.
Provider sandbox tests are separately gated and never imply live-money acceptance.
Mercury sandbox IP restrictions affected earlier validation; check current
provider access separately from local tests.

Historical verification results and the schema-policy timeout investigation are
preserved in [the October verification record](docs/verification-2026-10-20.md).
Those results do not establish the state of the current deployment.

## Model references

The model follows [TypeDB's RBAC in Business guide](https://typedb.com/docs/learn-typedb/rbac-in-business/).
Service composition follows the
[DogRS examples](https://github.com/Jitpomi/dogrs/tree/main/dog-examples),
especially fleet-queue and social-typedb. TypeDB functions alone are not an
automatic query authorization boundary: the protected data service always uses
dog-typedb's authenticated schema-policy executor.

## Stripe provider services

Stripe follows the same provider layout as Mercury. Each resource has its own
service, shared hook registration, hooks and schema. Shared authentication and
request-safety hooks are in `src/plugins/stripe/hooks/`. Credentials live only
in `auth-stripe`; the adapter contains HTTP/endpoint/form/error handling, not
TypeDB queries, invoice workflows or application business rules.

| Service under /stripe/ | Methods beyond find/get |
| --- | --- |
| customers | create, patch, remove, payment_methods |
| invoices | create, patch, remove (draft), finalize, pay, send, void, mark_uncollectible, attach_payment |
| invoice-items | create, patch, remove |
| checkout-sessions | create, patch, expire, line_items |
| payment-methods | create (tokenized card), patch, attach, detach |
| setup-intents | create, patch, confirm, cancel |
| payment-intents | create, patch, confirm, cancel, capture |
| invoice-payments | read-only |
| charges | patch; use PaymentIntents instead of deprecated charge creation |
| refunds | create, patch, cancel |
| disputes | patch, close |
| mandates | get only |

Configure `STRIPE_CONNECTIONS_JSON` as an external secret containing an array of
connections, each with `tenant_id`, `identity_ids`, `environment` (`test` or
`live`), `token`, optional `connected_account`, and `write` (defaults false).
No fallback global key is granted to every tenant. Configure each tenant against
its own Stripe account/Connect account: JWT tenant checks do not partition
customers within a shared upstream Stripe account. API keys are server-only.
A configured Connect account cannot be overridden with client headers.
The existing API version `2026-03-25.dahlia` is preserved.

All resource calls reauthenticate using dog-auth, resolve credentials through the
registered auth-stripe hooks, and run resource validation. Auth-stripe has no
public route. Use a Bearer token and `x-tenant-id` for HTTP calls. Custom methods
use POST to the collection with `x-service-method` and an `id` in the JSON body.
Stripe partial updates use PATCH, not PUT. Unsupported HTTP methods return 405.

Every provider POST requires a stable `Idempotency-Key`. It is namespaced by
tenant, environment, Connect account and endpoint; retries of the same operation
must keep the original key and body. Stripe's retention window is not permanent
exactly-once delivery. GET and DELETE do not send the key. There are no automatic
write retries or redirects. Reconcile uncertain outcomes before retrying. Amounts
are integer minor units, never floating-point money. Raw card/bank credentials
are rejected; collect them with Stripe-hosted UI or Stripe.js.

Find returns one page, not a settled total. Use `limit` (1–100) and the last
returned ID as `starting_after`; continue until an empty page. Custom list
methods preserve Stripe's list envelope and `has_more`. These are provider
proxies, not a completed storefront, webhook receiver or fulfillment workflow.

Tests live in `tests/stripe/`:
`cargo test --locked --lib stripe` exercises registered JWT/auth/resource hooks,
resource/action endpoint mapping, tenant separation, validation, idempotency and
real local HTTPS transport. The ignored `real_stripe_test_customer_lifecycle`
requires `BUSINESS_ENV_FILE` pointing to an external dotenv file with
`STRIPE_SECRET_KEY` in TEST mode. It creates synthetic resources only, never
confirms a payment, sends an invoice, or uses real money. Real sandbox coverage
does not imply live-account or all-action acceptance.


## Mercury provider services

Mercury follows the resource-service/shared-adapter pattern. Resource services
delegate to `src/plugins/mercury/adapters/mercury_adapter.rs`; that adapter makes
Mercury HTTP calls only. It contains no TypeDB queries or billing workflows.

| Service | Methods |
| --- | --- |
| `mercury/accounts` | `find`, `get` |
| `mercury/customers` | `find`, `get`, `create`, `update`, `remove` |
| `mercury/invoices` | `find`, `get`, `create`, `update`, custom `cancel`, custom `pdf` |
| `mercury/transactions` | `find`, `get` |

Mercury's POST edit endpoints require complete resource data. They are **update**,
not patch. Invoice updates must explicitly retain or clear optional fields.
Neither accounts nor transactions expose mutation methods. Customer deletion
requires one UUID; bulk deletion is forbidden. Invoices are cancelled, not deleted.
Account `get` searches the documented paginated accounts API: it does not invent
an undocumented single-account endpoint. `find` returns one page of records;
use `limit` (1–1000), `order`, `start_after` or `end_before` to page explicitly.
Transaction filters are documented in `transactions/transactions_schema.rs`; array filters use
comma-separated values in REST query parameters and become repeated upstream keys.

Each resource registers strict dog-auth JWT authentication, the
`auth-mercury` credential-resolution hook, and its own request/response validators.
The resource's `_schema.rs` defines its contract and its `_hooks.rs` invokes those
checks. There is no shared Mercury resource-validation dispatcher.
Mercury-wide resource security hooks live in `mercury/hooks/`: one registration
helper installs strict Business authentication followed by credential resolution
through the registered `auth-mercury` service. Resource validation follows these
shared hooks; credential storage and auth-service-specific hooks stay in
`auth-mercury/`.
Internal calls require a fresh bearer token too. No request can choose an API host,
provider token or model. Credentials are server-only and excluded from serialized
parameters and debug output. No global Mercury token is shared across arbitrary
tenants. Connection access is explicit and read-only unless configured otherwise.

`auth-mercury/` has its own service, hooks, shared definitions and schema. Its
server-only custom `resolve` method resolves the connection for the freshly
verified Business tenant/identity. Resource hooks call it through
`ctx.services.app().service(...)`, so the auth service's hooks run; they do not
access the raw service registry. It has no public HTTP route and explicitly
rejects external-provider calls. Tokens are returned only to the internal caller
and bound into non-serializable, debug-redacted request credentials. The Mercury
HTTP adapter owns neither the credential store nor resource request validation.
The existing connection mode is API-token authentication: these tokens are
configured by the operator, not obtained/refreshed through an invented OAuth flow.

Set `MERCURY_CONNECTIONS_JSON` in the external secret environment file to a JSON
array with `tenant_id`, `identity_ids` (array), `environment` (`sandbox` or
`production`), `token`, and `write` (default false). Use the exact verified identity
IDs and a token created in the corresponding Mercury environment. One connection
per tenant is currently supported. Omit this setting to leave Mercury unregistered.
This is deployment credential configuration, not a TypeQL query registry.

Standard methods use the corresponding REST verbs (`PUT` for full `update`).
Custom methods use `POST /mercury/invoices`, `x-service-method: cancel` or `pdf`,
and body `{ "id": "<invoice UUID>" }`, together with the normal bearer token and
`x-tenant-id` headers. GET cannot trigger cancellation. PDF results are a lossless
JSON envelope with `contentType`, `encoding: "base64"`, and `data`.

Creating an invoice requires an explicit `sendEmailOption` (`DontSend` or
`SendNow`), because Mercury otherwise sends it immediately. Requests do not round
money, silently merge partial updates, follow redirects, or retry mutations.
After an uncertain result, reconcile against Mercury before retrying. Do not
assume Mercury implements an idempotency guarantee that its endpoint does not
document. JSON and PDF response sizes are bounded; provider error bodies and
credentials are not exposed to callers.

Run `cargo test --locked --lib mercury` for the provider module tests, or
`cargo test --locked --lib` for all currently registered unit tests. Tests are in
`tests/mercury/` and exercise registered DogRS services, real JWT verification,
HTTP routing, and actual local HTTPS traffic. They never call Mercury or send
real invoices. A real sandbox smoke check needs a separately configured sandbox
token; local fixture tests are not evidence of live account/API access.

Check every backend target with `cargo check --locked --all-targets`, build with
`cargo build --locked --all-targets`, and run the registered backend suite with
`cargo test --locked --all-targets`. External sandbox and disposable TypeDB tests are
explicitly ignored by default and must be run separately with development
credentials; a green local suite is not proof of production acceptance.

References: [Mercury invoicing](https://docs.mercury.com/docs/invoicing),
[full invoice updates](https://docs.mercury.com/reference/updateinvoice),
[sandbox setup](https://docs.mercury.com/docs/using-mercury-sandbox).

This application uses published dog-typedb 0.3.4, dog-auth 0.3.1, and other DogRS 0.3.0 crates, dog-transport HTTP hosted by
Axum, and TypeDB 3.13.6 (the driver version required by dog-typedb).

Development and production use the same code, TLS validation, auth strategy and
durable TypeDB token store. Set `BUSINESS_ENV_FILE` to an owner-readable file
outside the project. The prepared configurations are in
`~/.config/jitpomi/business/dev.env` and `prod.env`. Their credentials must not be
committed or pasted into logs. Use a dedicated non-admin runtime account.

## Billing execution

`src/automation/` owns durable workflow execution and triggers.
`src/billing/` owns billing recurrence rules, consent, invoice state, collection claims,
receipts and reminders. `src/finance/` defines business-owned financial and
notification contracts. `src/plugins/` contains Stripe, Mercury and Resend
implementations and trusted configuration. See [plugin architecture and migration](FINANCIAL_PLUGINS.md).

The same canonical tenant/account graph backs protected `/data` and `/portal`.
Provider references, consent and credentials remain private. Credentials are encrypted in private connection records and excluded from
ordinary business records and API responses. Notification delivery does not require a financial plugin.

The durable automation engine owns execution and tenant-owned encrypted connections.
Enable `AUTOMATION_ENABLED=true` after its explicit schema installation and tenant
import; `BILLING_ENABLED=true` enables the portal and dynamic billing ingress.
New tenants configure their connections through `/automation` rather than changing
deployment JSON. See [automation API, security and cutover](AUTOMATION.md).

The previous process timer, external billing tick route and
`BILLING_WORKER_TENANTS_JSON` list are removed. A persisted `billing-cycle`
workflow invokes the existing business engine. Provider webhooks use
`/tenants/<UUID>/webhooks/<plugin ID>`; onboarding and recovery use that same tenant
prefix. `ACCOUNT_TENANT_ID` optionally mounts root aliases for the account website.
Connections are resolved afresh for the tenant; no header retargets a payment
account. Sandbox notifications still require an approved recipient.

Amounts are integer USD cents. Weekly/monthly/yearly schedules use UTC and retain
the original anchor (January 31 → February 28 → March 31). One-time plans finish
once. Schedule advancement and invoice insertion commit together. Tenant revision
writes arbitrate overlapping workers; every mutation requires exactly one result
before commit. A changed/missing consent revision, manual customer choice, manual
merchant policy or paused plan prevents automatic collection. Admin preference
alone never grants consent.

Stripe auto-advance is off. The worker makes at most one off-session attempt per
invoice, after a durable claim, using a stable idempotency key. Declines leave a
payment link for the customer. Processing bank payments receive no pay-again
reminder. Open due invoices can receive at most four reminder windows, separated
by the tenant's `reminder_days` (1–30, default 7). A paid invoice gets one receipt.
Resend acceptance is recorded with its message ID; acceptance is not itself proof
of inbox delivery.

### Uncertain outcomes and operations

Do not reset durable claims to retry blindly. An `issuing`/`attention` invoice with
no external ID needs provider-side lookup by `business_invoice` metadata (Stripe)
or `JIT-<invoice UUID>` (Mercury). A known draft needs operator reconciliation
before finalization; the worker does not recreate it. A Mercury customer marked
`pending` similarly needs reconciliation because customer creation has no assumed
idempotency contract. A notice in `sending` or `attention` must be checked with
Resend before any re-send. These states deliberately stop ambiguous external
side effects. They are not automatically repaired by restarting the worker.

Keep the old remote scheduler disabled before any cutover, reconcile outstanding
provider records, and configure the new worker against a verified canonical
database. Startup does not migrate data, configure a remote webhook, or enable
live payments. Stripe test acceptance does not certify Mercury's hosted card
checkout.

Mercury support (Sophie, ticket #2717152; confirmation received October 28,
2026) confirmed that card payments are not supported in Mercury's sandbox.
The missing card option is an expected sandbox limitation, even when Stripe is
connected and the invoice API returns `creditCardEnabled: true`. Do not reconnect
Stripe or change live settings to try to fix this behavior.

Use Mercury sandbox to check invoice creation, branding and supported API
behavior. Use Stripe test mode for successful and declined card payments,
automatic collection, receipts and reminders. Mercury's full hosted card flow
remains unverified until a separately authorized live transaction is completed;
simulated invoice status changes are not evidence of card processing. No live
transaction is authorized by running the sandbox tests.

### Sandbox workflow acceptance

Run the explicitly ignored `scheduled_payment_receipt_and_decline_reminder` test
in `tests/billing/main.rs` with `BUSINESS_ENV_FILE` (development operator source,
`TYPEDB_DB=business_dev`) and `BILLING_TEST_ENV` (protected Stripe test key and
approved Resend test configuration). It creates/deletes only a new
`business_backend_test_<UUID>` database. Optional `BILLING_TEST_REPORT` writes
non-secret evidence. Plaintext TypeDB is permitted only for loopback test
instances; normal runtime continues requiring TLS. The provider fixtures are
synthetic and remain in Stripe sandbox for inspection.

```sh
cargo test --locked --test billing -- --ignored --nocapture --test-threads=1
```

This test actually pays a $1 sandbox invoice, declines another, verifies the
receipt/reminder records, and checks repeated ticks, concurrent scheduling,
revoked consent, paused plans, provider binding, tenant isolation and signed
webhook replay. It sends clearly marked test emails only to dev@jitpomi.com.

### Sandbox deployment recovery — October 27, 2026

The free TypeDB Cloud validation cluster had automatically paused. Resuming the
existing cluster restored connectivity without changing its address, credentials
or stored data. `business_native_dev` passes canonical schema verification.
The deployed billing merchant must match the tenant in that database and the
provider connection configuration; legacy company IDs are not interchangeable.

Read-only provider connections remain available to authenticated provider reads,
but are not selected as billing executors. A read-only Mercury connection does
not prevent a writable Stripe test connection from starting the worker. Mode
mismatches on writable connections still fail startup. No permission is expanded.

The portal now adapts billing reads/writes to protected canonical data queries.
The approved sandbox accounts were migrated from `business_dev` without changing
the source database. Imported unfinished schedules are paused, payment consent is
cleared, and historical invoices are excluded from charging and notifications.
Backend health alone does not establish website acceptance.


### Sandbox invitations and password resets

Install the additive `account_links.tql` schema explicitly with
`BUSINESS_ENV_FILE=/private/development-operator.env cargo run --locked --bin install_account_links`.
This command only targets `business_native_dev`; startup never migrates data.
Merchant owners invite, renew or revoke through authenticated `/portal` actions.
Acceptance (`POST /onboarding`) and reset (`POST /recovery`) require the gateway
secret and are rate limited at the website. Password work has a separate bounded
admission budget. These handlers reject live provider mode.

Only SHA-256 digests of random invitation/reset tokens are stored. Invitations
expire in 48 hours; reset links expire in 30 minutes. Renewal/revocation changes
the buyer's invitation revision, so older links fail. Acceptance consumes the
link atomically with membership creation. Existing identities must supply their
existing password; an invitation cannot overwrite it. Reset consumes its link
and changes the credential version atomically, revoking existing sessions and
other outstanding reset links. Passwords use bcrypt. Reset requests return the
same message for known and unknown addresses, with a five-minute email cooldown.

Mail uses the tenant's independent notification plugin and exact sandbox
`allowed_test_recipients` list. All sandbox messages are marked TEST. Links point
to `https://jitpomi.com/account/`; the Cloudflare team gate stays in place. Mail
failure is explicit to the inviting owner; reset requests preserve the generic
response. A replacement invitation can be sent after checking the inbox.

The disposable database workflow also tests acceptance, renewal, revocation,
expiry, existing-identity protection, gateway access, tenant boundaries,
concurrent reset consumption and old-session invalidation. It never resets an
existing user's password.
