# Mercury sandbox acceptance

These checks use the current native TypeDB model, authenticated `/data` and
`/automation` services, tenant-managed encrypted connections, the durable worker,
the Mercury financial plugin and an independent Resend notification plugin.
No Stripe connection is configured. No production or active development database
is changed, and no provider status is manufactured in JITPOMI.

## Preservation and setup

Use the approved development operator configuration (`TYPEDB_DB=business_dev`),
the private sandbox credentials file, a private absolute test directory (mode
0700), and the preserved generated Mercury checkpoint directory. The checkpoint
contains `run.json`, schema and binary export files. Do not replace the original
backup or its records.

On first use, the test imports the checkpoint into generated reference and current
model databases. It compares the entire logical graph, including attributes and
relation roles, before adding the isolated test owner or automation configuration.
The reference copy is removed only after verification. The canonical test target,
original checkpoint, encrypted test secrets, local test authentication store and
new exports are retained. Credentials never enter TypeDB.

The test creates a separate random encryption key under the private test directory;
it does not rotate the application's existing master key or touch its secret store.
Subsequent runs reuse the same test database, connection aliases, plan key and
idempotency key. They do not recreate the invoice or resubmit a payment.

```sh
BUSINESS_ENV_FILE=/private/dev.env \
BILLING_TEST_ENV=/private/billing-test.env \
MERCURY_TEST_ROOT=/private/mercury-acceptance \
MERCURY_RESUME_CHECKPOINT=/private/original-mercury-checkpoint \
cargo test --locked --test billing mercury_managed_processing_checkpoint \
  -- --ignored --test-threads=1 --nocapture
```

The sandbox credentials file supplies `MERCURY_SANDBOX_TOKEN`,
`MERCURY_SANDBOX_ACCOUNT_ID`, `RESEND_API_KEY`, `BILLING_EMAIL_FROM`, and
`BILLING_TEST_EMAIL=dev@jitpomi.com`. The token must permit the test connection's
exact public egress IP. Never disable the restriction or use a production token.

## Phases

1. `mercury_managed_processing_checkpoint` reconciles the preserved payment through
   a tenant-connected Mercury plugin and an actual durable automation run. It
   verifies Processing produces neither another reminder nor a paid receipt and
   reusing an event returns the same run.
2. `mercury_managed_create_invoice_and_reminder` creates one new USD 1 invoice plan
   through native policies, then issues and reconciles its Mercury invoice through
   the worker. It checks the accepted reminder and event replay. Its invoice ID
   and provider URL are saved for the hosted sandbox payment step.
3. Complete that synthetic invoice's payment in Mercury's hosted sandbox flow.
   Never resubmit the preserved payment just because it is still Processing.
   While it is Processing, `mercury_managed_new_processing_payment` checks that
   reconciliation produces no repeated reminder or premature receipt.
4. `mercury_managed_paid_receipt_and_replay` observes the new invoice through the
   plugin and worker and requires Mercury to report Paid. Only then can the test
   pass with one accepted application receipt. A Processing response fails the
   full paid-receipt phase and preserves the checkpoint for a later rerun.

Select each test explicitly using the command above. Payment polling/notification
runs are checkpointed; do not run every ignored integration test indiscriminately.
Do not change `bill_status`, change a pending bank payment to Paid, delete notice claims,
replace an uncertain event key, or remove idempotency guards to make a test pass.

Reports under `MERCURY_TEST_ROOT` distinguish phase success, raw observed state and
provider-Paid/application-receipt verification. New immutable exports are written
under `checkpoint/`. Mail acceptance is not inbox delivery; separately verify the
invoice reference in the approved test mailbox and check that replay sends no
second message.

## Provider boundary

Mercury's invoice API reports payment state; it does not initiate the customer's
payment. Poll the provider invoice and payment records. Processing is not Paid.
The hosted flow warns that paying the originating account can take up to five days.
A provider payment acknowledgement is not proof of invoice settlement and must not
cause JITPOMI to send a paid receipt early.

References: [Mercury invoicing](https://docs.mercury.com/docs/invoicing),
[sandbox setup](https://docs.mercury.com/docs/using-mercury-sandbox), and
[applied invoice payments](https://docs.mercury.com/reference/listinvoicepayments).

## Separate manual-payment sandbox case

`mercury_managed_create_manual_payment_case` creates a distinct USD 1 invoice.
Use Mercury's supported **Mark as paid → Externally** workflow to record a clearly
simulated manual payment against that separate sandbox invoice, then run
`mercury_managed_manual_paid_receipt`. Confirm the provider invoice is Paid and
`GET /ar/invoices/{id}/payments` contains one payment of type `manual`.
This exercises the real Mercury Paid response and the application's notification
plugin; it is not evidence of settlement for either pending bank payment.
Never use this step to relabel or replace the pending hosted-payment test.

Rerun the manual paid test in another process to verify persistent connection
secrets, durable notice claims and one receipt after restart/replay. Confirm the
approved mailbox contains one reminder and one application receipt for that exact
invoice reference. The provider receipt and application receipt are distinct.
