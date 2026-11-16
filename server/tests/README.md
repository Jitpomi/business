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
real authentication, schema policy, the polling worker, backup and installer tools.
`BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test plugins two_tenants_complete_billing -- --ignored`
runs complete billing workflows through the automation connector and independently
resolved bank/mail capabilities for two tenants. Neither sends a real payment or
email. Real Stripe/Mercury acceptance remains separately gated.
