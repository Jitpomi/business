# Tenant financial and notification plugins

Business policy belongs to JITPOMI. External services translate protocols; they
must not decide who owns an invoice, whether consent exists, or when to charge.

## Boundaries

- `finance::FinancialPlugin`: normalized customer creation, invoice creation,
  finalization, observation, collection, saved methods, history and webhook events.
  Capabilities describe supported operations. Unsupported operations fail explicitly.
- `finance::NotificationPlugin`: delivers a business-owned message with a stable
  delivery key. It has no dependency on a financial provider or payment credentials.
- `finance::TenantRuntime` / `TenantRegistry`: tenant-scoped plugin instances and
  delivery policy. Lookups never borrow another tenant's credentials or default.
- Native TypeDB functions decide consent, ownership, collection eligibility, notice
  eligibility, terminal-state transitions and the next billing sequence.
- `billing::engine::Engine` orchestrates provider calls and submits atomic native
  claims. `dog-queue` computes anchored calendar dates and owns execution, leases
  and scheduling; the engine does not run a separate billing timer.
- `plugins`: built-in adapters, HTTP transport, signatures, provider history and raw
  provider services. Vendor payloads and credentials stop at this boundary.
- `/data`: account discovery is a frontend TypeQL query using native context
  functions. Discovering one's companies does not widen tenant data access.
- `services::financial`: tenant runtime metadata and provider operations (saved
  method listing/setup and payment history), authorized by native billing policies.
- Native TypeDB policies own customer and invitation creation, renewal/revocation,
  payment-method choice, explicit consent and removal intent. Actor and time come
  from the authenticated data context, not the caller.
- Authentication plugins own credential-bearing challenges, managed-provider
  acceptance and notification handoff. Identity/account materialisation is in
  `auth::AccountLifecycle`; credentials stay outside TypeDB.
- Reconciliation is an existing `billing.reconcile` automation action. There is
  no `/portal` service, route, redirect or compatibility dispatcher.

Only the financial plugin bridge can create verified-method facts. Native queries
cannot manufacture them. Detach receives the expected external customer as well
as the method and stable idempotency key; adapters must verify that binding and
support safe reconciliation of a lost acknowledgement. Native removal intent is
committed first, and a provider failure leaves the method unavailable for consent.
Confirmation saves a method without implicitly changing a default or authorizing
charges. Consent is a separate atomic native transaction, with server-owned
`consent_at` and identity evidence. The older integer audit field is retained for
historical compatibility; it does not establish authorization or policy time.

Customer bindings are a private map from plugin ID to opaque external reference;
selected methods record their plugin ID. Removed methods are namespaced by plugin.
Invoice records retain the plugin ID used at issuance, so changing a tenant's default
never reroutes existing invoices to another provider. Tokens are not stored in this map.

These are trusted in-process Rust plugins, not downloaded executable extensions.
To integrate another financial system, implement `FinancialPlugin`, register an
instance under a stable ID in its tenant runtime, and inject the registry with
`app::build_with_plugins`. Implement `NotificationPlugin` independently for email,
SMS or another delivery system. Normal startup's composition root currently builds
Stripe/Mercury and Resend from private environment JSON. Add other adapter factories
there; the engine and public services need no vendor-specific branches. Never reuse a plugin
ID for a different account while historical bindings still reference it.

The plugin contract accepts opaque references and normalized states. Adapters must
validate their own protocol, environment, account and response completeness, and
honor stable idempotency keys where supported. Report capability limits truthfully.
Customer creation without idempotency is durably claimed before an external call;
ambiguous results require reconciliation. Invoice IDs are persisted before optional
finalization; collection and notification claims survive restarts. Do not reset
claims to retry an uncertain external side effect blindly.

## Configuration

Managed runtime connections now live in the encrypted tenant connection registry.
See [automation architecture, API and required cutover](AUTOMATION.md). The previous
deployment JSON loader remains an explicit import/test/standalone-proxy boundary;
it does not supply managed runtime credentials. Tenant financial capability
contracts and adapters are preserved. New connector packages can supply those
capabilities without editing billing rules.

The engine discovers due work from persisted workflow schedules. The old billing
timer, tick HTTP endpoint, scheduler secret and worker tenant list no longer drive
execution. Tenant webhook/account routes resolve managed connections per request.

Retain old tenant JSON only in the private import source until cutover is verified.
The managed runtime does not fall back to deployment credentials.

Mercury `card_enabled` and `ach_debit_enabled` default to false. Bank-transfer
invoices work without Stripe. Mercury card acceptance requires connecting Stripe
inside Mercury; that is a provider feature requirement, not an engine dependency.

## Explicit migration of an existing canonical database

Startup never migrates an existing database. New databases use `billing.tql` directly.
Before upgrading an existing canonical database:

1. Stop every old worker, scheduler and writer. Back up schema and data, verify the
   backup can be restored, and record the exact target database and outstanding claims.
2. Prepare private operator credentials with an explicit `TYPEDB_DB`. Review
   `src/plugin_migration.rs`. From `server`, run:

   ```sh
   BUSINESS_ENV_FILE=/private/operator.env cargo run --locked --bin migrate_financial_plugins -- --apply
   ```

3. The migration expands the schema, copies legacy customer references (including
   pending claims) into plugin bindings, records the legacy selected-method plugin,
   and copies removal protections into namespaced tombstones. It retains the old
   hidden reference attributes as an archive, all invoices, plans, consent, revisions,
   notices and account links. Conflicting existing bindings abort instead of guessing.
   Copying is transactional; schema expansion is resumable and rerunning is safe.
   New required fields are enforced only after successful copying.
4. Verify record counts, retained references, consent and paid history. Configure
   tenant plugins, workers, secrets and website root aliases. Start the upgraded
   application with least-privilege runtime credentials, then re-enable schedulers.
   Reconcile pre-existing uncertain claims with the relevant provider before retries.

On 2026-11-15, `business_native_dev` was explicitly backed up, restore-verified
and migrated while its hosted writer was suspended. All 127 existing entities and
relations, their original attributes and every model count were verified unchanged.
`business_dev` (the older model) and production were not migrated. Rollback after accepting new writes
requires a planned data migration or restoring a verified backup; retaining old fields
alone does not make an old binary compatible with newly created customer records.

## Verification and current limits

`cargo test --locked --all-targets` covers vendor transport/authentication protections,
business rules, independent notification delivery, plugin isolation and architecture.
Database tests require explicit development credentials and create/delete only their
own generated database:

```sh
BUSINESS_ENV_FILE=/private/development-operator.env cargo test --locked --test plugins -- --ignored --test-threads=1
```

The custom bank fixture runs two tenants through real TypeDB storage: scheduling,
successful and declined collection, receipts, reminders, replay protection and
cross-tenant rejection. It also exercises authenticated financial-plugin dispatch through the
injected registry. A separate test checks legacy data migration and repeatability.

This demonstrates a third plugin without changes to business logic; it does not
certify arbitrary banks or live payment processing. Current product amounts remain
integer USD cents. Mercury's built-in adapter supports manual invoice flows, not saved
methods or automatic collection. Public account-link handlers retain their existing
sandbox-only activation guard. Stripe and Mercury sandbox/live certification remains
separate: the earlier Stripe end-to-end run stopped at portal authentication and
Mercury access was blocked by its IP restriction. Those earlier runs were not passing.
On 2026-11-15 the full Stripe sandbox acceptance test passed through the new plugins,
including the portal and account workflows. Mercury billing preflight initially returned HTTP 401 before creating records.
After the user approved adding the exact test-egress IP to the existing sandbox
allowlist, preflight, customer/invoice creation, normalized reconciliation, hosted
invoice/PDF and reminder replay checks passed without a Stripe connection. The
authorized USD 1 hosted payment was submitted and Mercury generated a matching
receipt, while the invoice API reported `Processing`. A real sandbox test verified
that this state preserves the initial reminder and produces neither another reminder
nor a premature settlement receipt. Final `Paid` reconciliation and the application
receipt remain pending provider settlement. A restore-tested private checkpoint
preserves the original invoice for later verification.
