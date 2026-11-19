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

## Native planning and attestation follow-up

A real development form request reached the transport timeout. Read-only profiling
of the same open invoice isolated native eligibility planning: the original
origin disjunction took 7.8–8.5 seconds per check. Keeping the same approved-origin
condition in a dedicated TypeDB predicate reduced that check to 2.4 seconds.
Both pre-provider and post-provider native checks remain in place. No timeout,
transaction deadline, origin approval or buyer permission is relaxed.

Capability producers now reuse only an exact current attestation under the shared
native freshness policy. They resolve current connection configuration first,
compare all capability flags and the complete currency set in TypeDB, and write
only when the facts differ or expire. Unchanged facts no longer rewrite the
merchant fence and conflict with a billing cycle. Viewer credentials are not
resolved before native access has selected the authorized merchant.

The explicit successor for these function changes is `business_native_dev_v10`.
Deployment must follow the same fresh checkpoint, isolated rehearsal, graph
verification and disabled-worker warmup procedure. All existing order, payment,
refund and notification journals remain unchanged by the migration.

The development acceptance also identified overlapping startup cycles. One worker
per tenant in this service now drains every original occurrence through dog-queue,
with four global slots retaining parallel execution across tenants. No occurrence,
uncertain attempt, or payment journal is discarded, and native fences remain in
force across processes and user requests.

## Deployed acceptance — 19 November 2026

Business PR #42 is merged and development runs commit
`74ed063a72ab40f646fffe176bef729f23b5ecff` against
`business_native_dev_v10`. Website PR #13 is deployed. Stripe fields mount in
Light DOM, slotted into the widget: Stripe does not support mounting its payment
element inside a ShadowRoot. The surrounding widget keeps its isolated styling;
scoped provider styles, exact origin/nonce/popup bindings and CSP remain enforced.

The final browser acceptance used a new synthetic tenant and its own separately
encrypted Stripe and notification connections on an approved external website.
The buyer signed in through Auth0, reviewed native pricing, confirmed one USD 1.00
order and paid using Stripe’s documented test card inside that external widget.
The widget showed paid only after native reconciliation. The merchant then
requested one USD 0.40 refund through the account UI; the tenant plugin executed
it and native reconciliation recorded success. Independent Stripe observation
found exactly one matching invoice, one paid payment and one matching refund.
No payment status was overridden and no raw provider payment/refund fallback was
used. The preceding deployed acceptance also verified a declined card without a
paid state or payment receipt.

The test mailbox received exactly one reminder, payment receipt and refund
receipt for the final invoice. Native scope checks for its actual buyer allowed
their own invoice, filtered a foreign invoice, denied a seller refund and left no
unauthorized proposal. Earlier invoices, payment/refund IDs and notification
acceptance IDs remained unchanged across cutover.

Before import, the drained database and independently encrypted offsite backup
matched all 6,973 original records and the complete relationship graph. Local
rehearsal and application-cluster verification preserved existing plan approvals.
The old development generation and recovery shadows remain retained; only its
active tenant statuses were fenced. A separate offsite restore verified all 20
pre-existing connection credentials, wrong-key rejection and wrong-tenant
rejection. The master encryption key is unchanged.

Final code validation passed correctness CI, the full TypeDB 3.14.1 gateway,
dog-auth and dog-queue lifecycle suite (549.87 seconds), the focused TypeDB 3.15
suite, native policy checks on both versions, strict Clippy and 65 website tests.
After the final startup catch-up window, all seven operational counters were zero.
Real payments remain disabled and Mercury refunds remain unavailable. The three
existing TypeDB chrono dependency findings remain visible in the separate security
audit; this release does not resolve or suppress those upstream findings.

The post-payment encrypted backup (Actions run `44521008004`) independently
restored 7,322 records and their full graph. The downloaded archive contained the
paid invoice, successful USD 0.40 refund and exactly one of each accepted notice.
The operator drill decrypted all 22 referenced connection credentials from 26
ciphertext rows without contacting the source secret store, and rejected both a
wrong key and a wrong tenant. Its isolated recovery shadow remains retained.
The scheduled backup now targets v10 with the matching encrypted runtime capsule.
