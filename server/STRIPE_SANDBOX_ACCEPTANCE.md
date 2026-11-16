# Stripe sandbox acceptance

These explicitly gated tests exercise the current tenant-managed connections,
TypeDB policies, authenticated `/data` and `/automation` services, durable worker,
Stripe financial plugin and independent Resend notification plugin. Environment
provider registries are empty. No Mercury connection is used.

Use the approved development operator configuration, private test credentials and
a new private absolute directory (0700). The credentials must include a Stripe
`sk_test_` key, `RESEND_API_KEY`, `BILLING_EMAIL_FROM` and
`BILLING_TEST_EMAIL=dev@jitpomi.com`. Credentials are encrypted outside TypeDB;
test authentication also has a separate private store. The test checks the graph
contains neither provider key. Original databases and Mercury checkpoints remain
untouched. Keep the generated database, `run.json`, encryption key, secrets and
immutable phase exports so the same cases can resume after restart.

```sh
BUSINESS_ENV_FILE=/private/dev.env \
BILLING_TEST_ENV=/private/billing-test.env \
STRIPE_TEST_ROOT=/private/stripe-acceptance \
cargo test --locked --test billing stripe_managed_create_invoices_and_reminders \
  -- --ignored --test-threads=1 --nocapture
```

Run phases individually, in order, with the same private directory:

1. `stripe_managed_create_invoices_and_reminders`: creates three synthetic tenants
   and buyers, one primary USD 1 invoice each, and one reminder each. The tenants separately
   configure identically named `my-finance` and `my-mail` connections through the
   authenticated API; tenant B cannot inherit A's configuration. Both use the
   available test Stripe account, so this is tenant isolation coverage rather than
   proof of two independently owned Stripe accounts.
2. `stripe_managed_pay_and_receipt`: collects those same invoices through the
   worker and tenant-selected financial plugin. Stripe's `pm_card_visa` succeeds;
   `pm_card_chargeCustomerFail` declines. The successful invoice must have one
   paid provider payment and one application receipt. The declined invoice must
   remain open with a payment link, one reminder and no paid receipt.
3. `stripe_managed_restart_replay`: run in a new process against the same records.
   New automation events must not repeat collection or mail. Reusing each event
   key must return the same run. Provider attempt counts remain one, and the
   invoice count remains one per plan.

`stripe_managed_consent_security` runs separately against the checkpoint. It
checks missing/expired attestation, pending/inactive methods, manual policy,
paused plans and later scope expansion. Every rejected/filtered write must leave
the complete logical graph unchanged. A valid grant then uses the public query
template. This phase verifies provider facts only; it sends no invoice, payment
or email.

Only test payment-method attachment uses the raw Stripe transport. Provider
customer setup resolves the tenant's managed financial plugin. Saved methods are
verified through that plugin and attested by the trusted capability. Plan creation uses native `/data` writes and TypeDB policy enforcement.
Distinct synthetic seller and buyer identities perform onboarding. The public
`/financial` capability verifies the tenant plugin's saved method; native `/data`
writes select it and insert an immutable, plan-scoped buyer consent grant. No
pre-consented customer flag or direct grant write bypass remains in this test.
See `NATIVE_PAYMENT_CONSENT.md` for the authoritative model and migration.

A third case revokes consent before collection, rejects restoration of its old
revision and obtains a new grant for future work. The old invoice remains unpaid
with zero payment attempts. A separate unapproved-plan control invoice remains
manual and uncharged even while another plan has consent. That control receives
no notification in this targeted acceptance run.

Each phase verifies the actual Stripe invoice's test mode, amount, currency,
customer binding, local invoice metadata and selected connection alias. It also
checks authenticated cross-tenant automation requests fail and TypeDB hides and
rejects mutation of the other tenant's invoice/customer. Results and raw observations
are saved privately before assertions. Database exports preserve every worker
phase, including failed assertions. Never force an invoice to Paid, replace an
uncertain payment key, erase a notice claim or delete the checkpoint to make a
failed test pass.

Separately inspect the approved mailbox: provider send acceptance is not proof
of delivery. Expect three primary reminders and one receipt for these exact invoice IDs;
verify restart produces no duplicate. Save Stripe's actual payment record and
charge receipt link. No hosted browser checkout or real bank settlement is
claimed by these API-based sandbox collection tests.

References: [Stripe testing](https://docs.stripe.com/testing),
[pay an invoice](https://docs.stripe.com/api/invoices/pay),
[invoice payments](https://docs.stripe.com/api/invoice-payment).

`stripe_managed_consent_status` checks the public customer-list template for both
buyer and seller accounts against the preserved checkpoint, without payment or mail.
