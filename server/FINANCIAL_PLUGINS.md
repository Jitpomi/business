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
- `billing::engine::Engine`: schedule advancement, consent, ownership, amounts,
  durable claims, one-attempt collection, terminal states and normalized reconciliation.
- `plugins`: built-in adapters, HTTP transport, signatures, provider history and raw
  provider services. Vendor payloads and credentials stop at this boundary.
- `services::portal`: authenticates and resolves ownership before selecting a tenant
  runtime. Existing `setup_card` / `confirm_card` actions remain aliases for generic
  `setup_method` / `confirm_method` actions.

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
there; the engine and portal need no vendor-specific branches. Never reuse a plugin
ID for a different account while historical bindings still reference it.

The plugin contract accepts opaque references and normalized states. Adapters must
validate their own protocol, environment, account and response completeness, and
honor stable idempotency keys where supported. Report capability limits truthfully.
Customer creation without idempotency is durably claimed before an external call;
ambiguous results require reconciliation. Invoice IDs are persisted before optional
finalization; collection and notification claims survive restarts. Do not reset
claims to retry an uncertain external side effect blindly.

## Configuration

See `.env.example`. All JSON is trusted deployment configuration, never request data.
Use one financial settings entry per tenant. Defaults are explicit; tenants can have
no financial plugin and still use accounts or notifications. Built-in connection
arrays retain their tenant, identity, write and environment restrictions.
`sandbox` defaults to true per tenant. Financial and notification credentials are
separate. Duplicate plugin registrations/settings and unknown defaults fail closed.

Workers require an explicit `BILLING_WORKER_TENANTS_JSON` list. Tenant routes are
`/tenants/<UUID>/internal/billing/tick`, `/webhooks/<plugin>` (under that same prefix),
`/onboarding` and `/recovery`. `ACCOUNT_TENANT_ID` adds root aliases for one listed
tenant to preserve the current website URLs. Configure each Stripe endpoint with
that tenant's secret. The scheduler Worker now requires `BILLING_TENANT_ID` and its
matching `BILLING_TICK_SECRET`; run separately configured schedulers as needed.
The template intentionally omits a tenant UUID: set the intended tenant before deployment.
Mercury settings `mercury_card_enabled` and `mercury_ach_debit_enabled` default to
false. Virtual bank-transfer instructions work independently of Stripe. Opting into
card acceptance requires a connected Stripe account within Mercury; that is a
Mercury feature requirement, not a requirement for using the Mercury plugin.

Old global billing merchant, payment mode, webhook and mail variables no longer
configure the runtime. Replace them with the tenant entries; do not leave deployment
operators relying on the old settings.

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
cross-tenant rejection. It also exercises authenticated portal dispatch through the
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
hosted payment step is awaiting user submission; payment/receipt certification
remains pending. A restore-tested private checkpoint preserves the pending test.
