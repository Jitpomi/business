# Inline checkout acceptance

The provider form belongs to an existing manual invoice. No additional invoice,
order or PaymentIntent is created by asking for a form. TypeDB enforces buyer
access, the current invoice state, merchant approval of an exact HTTPS origin,
fresh sandbox capability/currency facts and bounds on the remaining amount due.
The provider balance is distinct from the immutable invoice total.

The financial service checks provider bindings, then repeats native eligibility
after provider I/O. Stripe uses the same invoice/intent observation as ordinary
reconciliation, including payments still processing. Confirmation secrets stay
out of TypeDB and tab persistence. OAuth uses the platform publishable key and
connected-account ID, following current Stripe guidance; deprecated OAuth keys
are unused. Existing connections without a public key retain hosted payment.

Widgets keep Auth0/account proofs inside the authenticated top-level checkout.
Only the selected provider form reaches the approved site through an exact
origin, popup and fresh nonce channel. Callback messages never establish payment.
The existing dog-queue/native billing lifecycle confirms settlement, refunds and
receipts. Unknown submission outcomes disable repeat submission. CSP additions
are limited to checkout/preview pages; cookie protections and frame-ancestor
restrictions remain intact. Widget styling uses an external stylesheet.

## Verification before development cutover

- Adversarial native commerce policy checks passed on TypeDB 3.14.1 and 3.15.0,
  including buyer/foreign/merchant boundaries, origin approval and revocation,
  immutable origin records, disabled capability facts, invoice state and amount
  bounds. Logs and generated fixtures remain in the private operator evidence.
- The full gateway/dog-auth/TypeDB 3.14.1/dog-queue regression passed in 549.48s:
  one-time purchase, wrong-currency rejection, processing without receipt,
  exactly one payment receipt, refund lost-acknowledgement recovery, instalments,
  recurring donations, consent, restart and mixed carts with independent states.
- The focused final inline gateway check passed on TypeDB 3.15.0 in 71.49s. It
  exercised approved and revoked origins, foreign and merchant denial, hosted
  form access, invoice reuse and absence of provider secrets in native records.
- Rust all-target tests, strict Clippy and 64 website tests passed. Frontend
  checks include live-key rejection, exact handoff bindings, omission of account
  credentials from the form projection, unknown-outcome handling, and preserved
  encrypted login context.
- An isolated rehearsal imported the verified v8 offsite checkpoint into the new
  model. All 3,502 original object values, relationship roles and plan-approval
  decisions were preserved. The application cluster was not written by rehearsal.

The explicit development target is `business_native_dev_v9`. Live Stripe form
acceptance and final deployment/recovery evidence must be recorded after the
controlled cutover; the synthetic gateway is not claimed as a real provider test.

## Cutover procedure

Stop development writers and verify a fresh encrypted offsite checkpoint before
merging and importing the shadow. Verify its complete graph and approvals before
retiring the old copy. Keep both `AUTOMATION_ENABLED` and `BILLING_ENABLED` false
when resuming Render and deploying the new commit. Confirm that commit is live
before enabling the flags and deploying the same commit again. This avoids
starting and interrupting old compatible workers during Render's resume step.
Deploy the frontend after the backend is active. Refresh the backup database and
encrypted runtime capsule, test actual sandbox payment/refund reconciliation,
then verify a fresh offsite checkpoint. Preserve source copies and all attempts.

Real payments remain disabled. Mercury refunds remain unavailable: its published
invoice API does not document a refund operation. A bank refund payout would be
a separate capability with verified destination and approval requirements, not
an invoice-status override or an undocumented transfer workaround.
