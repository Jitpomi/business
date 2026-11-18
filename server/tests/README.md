# Backend tests

Tests are separate from source, following the DogRS/sp-connector service layout.

- `backend/`: real TypeDB policy and main-server HTTP tests, plus executable provisioning/bootstrap/login/data/logout acceptance. Uses the same app composition as production; no alternate server.
- `mercury/`, `stripe/`: registered resource/auth hooks, tenant boundaries, HTTP transport and separately gated real sandbox checks.
- `billing/`: canonical scheduler and real sandbox payment/receipt/reminder acceptance (explicitly gated); source-adjacent tests are linked from this directory.
- `automation/`: encrypted vault, bounded mappings, HTTP destination policy, real tenant/API/worker isolation, replay/approval/recovery and additive installer/backup checks.
- `unit/`: current configuration, database-tool validation, token keys, admission and data envelopes.
- `frontend/`: the actual frontend TypeQL client and binding contracts.
- `layout/`: every Rust source/test must be reachable from a Cargo target,
  and each service must have its common service/shared/hooks/schema modules.

Run `cargo test --locked --all-targets` and
`node --test tests/frontend/data-client.test.mjs ../deploy/automation-wake/worker.test.mjs`.
Real integrations are intentionally ignored by default, not empty placeholders:

```sh
BUSINESS_ENV_FILE=/private/dev.env BUSINESS_RUNTIME_ENV_FILE=/private/runtime.env cargo test --locked --test backend -- --ignored --test-threads=1
```

The first file supplies existing development-cluster operator credentials with
`TYPEDB_DB=business_dev`; the second supplies non-admin runtime credentials for
the same cluster. Tests create only generated disposable databases and never reset
configured databases. Provider checks must be selected separately; never run all
ignored tests with production credentials. A provider proxy test is not evidence
that recurring billing, onboarding, recovery or fulfillment workflows exist.

## Real-provider HTTP acceptance

`tests/acceptance/provider-http.mjs` exercises the deployed validation service or
an explicit loopback instance, using real owner login, tenant hooks and provider
HTTP. Supply `BUSINESS_TEST_BASE_URL`, `BUSINESS_TEST_TENANT`,
`BUSINESS_TEST_OWNER_EMAIL`, `BUSINESS_TEST_OWNER_PASSWORD` from a private
environment, and select `BUSINESS_TEST_PROVIDER=stripe` or `mercury`.

Stripe additionally requires `BUSINESS_ALLOW_PROVIDER_TEST_WRITES=1` and an
explicitly registered TEST connection. It verifies synthetic customer CRUD and
idempotency, unconfirmed setup/payment-intent cancellation, Checkout expiration,
draft invoice/item reads, cleanup, tenant denial and logout. It never confirms a
payment, finalizes an invoice or sends email. Mercury performs read-only checks of
all four resources and fails if upstream access is denied; an allowlist failure is
not reported as a passing end-to-end test. No provider token is sent by this test
client or printed in its output.

## Account and invoice acceptance without providers

To exercise the canonical account and local invoice lifecycle against a generated
disposable development database without sending email or contacting a payment
provider:

```sh
BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test billing account_and_invoice_workflow_without_provider_effects -- --ignored --test-threads=1
```

This test disables mail and clears its process-local provider connections. It
covers real HTTP account flows, recovery races, scheduling and verified local
state transitions using synthetic provider observations. It does not certify
Stripe/Mercury connectivity or live billing. Run it separately from the explicit
provider-and-mail acceptance test because those tests configure process-global
environment variables differently.

Mercury's billing-plugin acceptance test is explicitly gated:
`BUSINESS_ENV_FILE=/private/dev-operator.env BILLING_TEST_ENV=/private/billing-sandbox.env MERCURY_WAIT_FOR_PAYMENT=true cargo test --locked --test billing mercury_manual_invoice_payment_receipt_and_reminder -- --ignored --nocapture --test-threads=1`.
It preflights sandbox destination access before creating anything, uses no Stripe
connection, issues a synthetic USD 1 invoice, checks accepted reminder delivery,
and waits up to ten minutes for payment through the sandbox's hosted experience.
Only an observed provider-paid state can produce the receipt assertion. A timeout,
permission error or unsupported sandbox payment flow is a failure, never a pass.
The generated local billing database is deleted afterward; provider test records
remain as evidence and must not be confused with real customer work.

For an interactive Mercury payment, save a verified test checkpoint while its writer
is quiesced and copy the awaiting-payment report to `run.json` in that directory.
Resume the same acceptance test with `MERCURY_RESUME_CHECKPOINT=/private/checkpoint-directory`
and a fresh `BILLING_TEST_REPORT` output path. It restores only into a newly generated
UUID database and reuses the original tenant/invoice bindings and accepted-reminder
record. It does not create another provider customer or invoice, or send the initial
reminder again. Only fresh Mercury-paid observation can advance receipt verification.

Use the separately gated `mercury_processing_payment_suppresses_notices` test with
`MERCURY_RESUME_CHECKPOINT` to verify a real provider `Processing` state without
creating a customer/invoice or sending notifications. A processing-only pass does
not certify final `Paid` settlement or application receipt delivery. The payer's
success screen and provider-generated receipt are not substitutes for invoice API
settlement state. The full test reports state changes and fails explicitly if paid
settlement is not observed in its bounded window.

## Automation acceptance

`BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test automation -- --ignored --test-threads=1`
uses only generated disposable databases and synthetic connector effects. It runs
real authentication, schema policy, dog-queue workers, competing TypeDB leases, stale acknowledgements,
restart/backfill preservation, backup and installer tools.
`BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test plugins two_tenants_complete_billing -- --ignored`
runs complete billing workflows through the automation connector and independently
resolved bank/mail capabilities for two tenants. Neither sends a real payment or
email. Real Stripe/Mercury acceptance remains separately gated.

## Authentication plugins and identity links

`authentication/` verifies signed asymmetric API tokens against a local HTTPS IdP,
issuer/audience/token-type/expiry rejection, authentication proof requirements,
explicit two-proof linking and TypeDB tenant isolation. It also verifies private
external account persistence and migration of encrypted plugin credentials out of
graph payloads. These tests do not claim live Auth0 configuration.

`backend/identity_tests.rs` exercises people without login accounts, multiple
external accounts for one person, per-account session invalidation, shared tenant
membership and protected identity links. Migration fixtures cover both older
credential models, pending recovery and revocations. Billing account acceptance
retains invitation/recovery races and first-account contact onboarding. Integration
tests explicitly enable a private temporary local-account plugin store; runtime
never enables it implicitly.

```sh
BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test authentication -- --ignored --test-threads=1
BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test backend identity_tests -- --ignored --test-threads=1
```

## Complete dependency-upgrade acceptance

The default suite intentionally excludes external-service tests. For the native
acceptance groups, provide the following operator inputs before using `--ignored`:

- `BUSINESS_ENV_FILE`: development TypeDB operator settings (`business_dev`).
- `BUSINESS_RUNTIME_ENV_FILE`: non-admin runtime credentials for that same cluster.
  The executable test explicitly selects its private local-auth fixture profile.
- `AUTH0_TEST_PROOFS_DIR`: fresh independent `synthetic-owner-proof.json` and
  `synthetic-outsider-proof.json` browser logins. Machine tokens do not replace
  the password/AMR browser test.
- `CONSENT_TEST_ROOT`: a fresh absolute directory with mode 700; migration
  evidence and refused/successful shadow databases are retained.
- `BUSINESS_WEBSITE_DIR`: the website checkout containing
  `portal/retirement.integration.mjs`.

Run the `backend`, `authentication`, `plugins` and `automation` integration test
binaries with `--ignored --test-threads=1`. These groups use generated test
databases and isolated provider fixtures. Run real-provider billing phases
separately according to the Stripe and Mercury acceptance guides, reusing their
checkpoint IDs. A missing operator input is a setup failure, not a passing test.

The acceptance suites run serially because they configure process-local plugin
profiles. Tests that change gateway, billing or automation settings capture and
restore their original values on scope exit, including error paths. Do not remove
the runtime gateway check or rely on another test to select an HTTP TLS provider.

Automation pollers return an owned Worker handle. Call `shutdown().await` to
stop new polls and finish the current poll before exporting or deleting a fixture.
The server drains its HTTP requests and worker on shutdown. Do not replace this
with task abortion, a sleep, repeated deletion, or replaying an uncertain effect.

## Queue cutover rehearsal

After exporting a fresh quiesced development backup, run
`BUSINESS_ENV_FILE=/private/dev.env cargo run --locked --bin rehearse_queue -- business_native_dev_v3 /private/backup`.
It restores into a generated development shadow, verifies the backup, adds only
missing queue deliveries and checks original objects, business attributes and
relation roles. A second backfill must insert nothing. No providers are called;
the source and backup remain untouched and the shadow is retained as evidence.
See `src/background/README.md` for execution and rollback responsibilities.

## Rehearse a retained checkpoint with current policies

The restore-only `policy_shadow_preserves_all_backup_work_and_is_idempotent`
test accepts `BUSINESS_RULES_TEST_SOURCE` alongside
`BUSINESS_RULES_TEST_BACKUP`. The source defaults to
`business_native_dev_v3`; an explicit retained generated checkpoint can be
selected instead. The existing development/source guards still apply. It
restores only into new UUID databases, compares records and relationship roles,
verifies additive policy backfill twice, and makes no provider calls. It never
changes the source database or its immutable backup.

## Commerce foundation

`BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test backend native_commerce_pricing_isolation_and_immutable_orders -- --ignored --nocapture --test-threads=1`
provisions a new UUID database and exercises the real scoped `/data` adapter.
It verifies catalogue ownership, immutable pricing and accepted order lines,
bounded donations, recurring terms, merchant-approved instalments/deposits,
integer rounding, discounts/shipping/tax, company isolation and revoked membership.
It makes no payment or mail calls and does not migrate the existing development
database. See `../docs/commerce-and-checkout.md` for the implementation boundary. This pricing test alone is not checkout certification.


## Shared checkout acceptance

`checkout_gateway_queue_payment_and_refund_recovery` in the plugins suite runs the
actual website Worker against a generated native database and tenant-owned bank/
notification connector. Set `BUSINESS_WEBSITE_DIR` to the website checkout and
`BUSINESS_ENV_FILE` to explicit development credentials. It covers quote versus
acceptance, scoped access, dog-queue invoices, processing versus settlement,
refund lost-acknowledgement recovery, independent order grants, finite instalment
rounding, recurring donations, an unapproved subscription and worker restart.
It counts provider submissions and notification sends, not only deduplicated rows.
Future clock tests select obligations through native functions; they never invent
provider settlement for a real bank or change a real payment's status.

`checkout_identity_enrolment_and_native_tenant_isolation` in authentication verifies
signed local HTTPS IdP proofs, verified contact, independent identities sharing an
email, private shops, buyer-only membership and revoked access.

`checkout_checkpoint_preserves_full_graph_and_existing_approvals` in backend takes
explicit `BUSINESS_CHECKOUT_CHECKPOINT` and `BUSINESS_CHECKOUT_SOURCE` inputs. It
restores only new UUID shadows and compares every original logical object,
attribute, relation role and existing payment approval. No providers or source
writes run. The CLI `migrate_checkout --rehearse SOURCE DIRECTORY` provides the
same preservation checks for hosted development before a quiesced `--apply`.

Stripe's managed suite adds `stripe_managed_checkout_refund`, after create and pay,
to prove a real TEST partial refund and one receipt through tenant plugins and
dog-queue. Keep new acceptance roots separate from old journals. Native consent
security still tests seller/foreign scope, forged methods/revisions/timestamps,
concurrent identical request IDs, revocation and unapproved future plans. Different
explicit request IDs may now approve different orders at the same revision.

All real database tests remain explicitly ignored by default. Local runs require
`BUSINESS_TEST_LOCAL_ONLY=true` and an explicit loopback test environment; this
flag cannot disable TLS for any remote address. Hosted acceptance uses verified
TLS. A local pass does not establish hosted deployment health or bank settlement.
