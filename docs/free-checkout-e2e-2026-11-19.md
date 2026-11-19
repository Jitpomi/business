# Free-service checkout acceptance — 19 November 2026

The main checkout scenarios passed against the deployed free-service environment.
Real payments remain disabled. No paid resources, runtime deployment, schema
migration or live credentials were introduced by this test. Production backup and
monitor switches remain false; development monitoring and daily encrypted backups
remain enabled. The production upgrade and its measured acceptance are still pending.

## Actual deployed flows

A new, separately journalled synthetic merchant used its own encrypted Stripe TEST
and notification connections, the deployed Business worker, native TypeDB policies,
dog-queue and the deployed website. Existing tenant/payment records were retained.

| Scenario | Observed result |
| --- | --- |
| External inline widget | Approved external origin -> secure JITPOMI popup -> hosted sign-in -> native price review/confirmation -> provider-owned card fields on the original page. |
| Declined test card | Clear decline error. Independent Stripe/native reads found the invoice open, zero paid payments, one reminder and no premature payment receipt. |
| Successful inline payment | USD 1.00 settled at Stripe; Business then reported Paid, removed payment submission and delivered one receipt. |
| Hosted checkout | Separate USD 1.00 order from the JITPOMI catalogue, paid on Stripe's hosted sandbox invoice page; provider and Business both confirmed Paid. |
| Partial refund | Merchant UI requested USD 0.40 for the widget invoice; native owner scope authorized it and the tenant's plugin/dog-queue completed exactly one refund. |
| Full refund | Merchant UI requested USD 1.00 for the hosted invoice; provider and native observations confirmed exactly one succeeded refund. Invoice Paid and refund status remained distinct. |
| Reminders and receipts | Exactly six matching inbox messages: one reminder, payment receipt and refund confirmation for each invoice. Native accepted-delivery journals and independent provider reads agree. |
| Native access boundaries | Own invoice visible, foreign invoice filtered, buyer-issued seller refund rejected, denied proposal absent. |
| Refund amount boundary | Native reserved refund equalled the full invoice amount. A further owner-scoped refund was rejected by TypeDB and left no proposal. The test schedule was paused for this negative test; no provider was called. |
| Preservation / repeated observation | Both new orders and all three retained Stripe fixtures still have one paid provider payment, one succeeded refund and three accepted notices each. No payment/refund was resubmitted to obtain these checks. |

The two new refund amounts were 40 and 100 minor units respectively. Actual inbox
messages arrived between 15:47 and 15:55 UTC. Payment details were entered only in
Stripe's browser fields, using documented sandbox cards; they never entered the
Business API or TypeDB. No financial status was manually overridden.

Initial page/quote loads were slow in this free-service run. The functional results
above do not establish a latency or availability SLA or certify paid capacity.

## Isolated broader lifecycle regression

The complete checkout gateway scenario passed in 551.11 seconds against real local
TypeDB 3.14.1, the actual website gateway, dog-auth and dog-queue, using isolated
generated data and independent synthetic payment/notification plugins. It covers:

- Catalogue, quote, confirmation and company isolation.
- Invoice issuance, processing without premature receipts, payment and one receipt.
- Native refund authorization and recovery after a lost acknowledgement, with
  exactly one provider refund submission.
- Logical-clock instalments, recurring donations and cancellation.
- Atomic mixed-cadence carts, separate invoices and scoped partial refunds.

All 70 frontend tests passed. The three deployed widget/checkout/payment-form
scripts matched the tested website source byte-for-byte. These broader synthetic
scenarios complement the two actual Stripe sandbox flows; they are not claims of
real bank settlement or of having waited through real monthly billing periods.
Mercury refunds remain unavailable, as previously agreed. This pass made no new
Mercury bank payment and does not claim to have completed its pending settlement.

## Test schedule pause and retained failure history

After both payments/refunds and all six accepted notices were verified, only the
new test merchant's sixty-second schedule was paused through native owner control.
Its cursor, original occurrences, invoices, refunds and notification journals were
preserved. Financial facts before/after the pause matched exactly.

One already-created occurrence then exhausted its retries with `Schedule paused;
occurrence retained`. Its stable occurrence key has no corresponding workflow run;
it never dispatched the billing workflow. The failed queue record remains intact.
This is the documented current pause behaviour, not a payment failure. The
cumulative `failed_jobs` metric is therefore **1**, not zero. Failed runs, overdue
jobs, financial attention, stale reconciliation, receipt attention and stalled
refunds remain zero. The monitor delivered its failed-job increase alert; history
was neither erased nor changed to successful to make readiness appear clear.

Keep this campaign paused. If resuming it later, review its retained occurrences
and cursor explicitly rather than blindly catching up old synthetic schedules.
Uncertain real provider outcomes must always remain withheld for reconciliation.

## Recovery and preservation

Fresh encrypted backup run `44560826075` succeeded. Its independent download,
authenticated decryption, complete graph/schema restore and vault verification
restored **10,978 graph records**, **28 ciphertext rows** and all **24 connection
credential references**, including the new synthetic merchant. The credential
verification never contacted the original secret store and rejected wrong-key and
wrong-tenant access. The isolated recovery shadow and original databases remain
retained. This is a development recovery rehearsal, not a production RTO claim.

Private browser screenshots, exact request/financial identities, mailbox message
IDs, native denials, original failed-job payload, source hashes and downloaded
recovery proofs are retained in the operator's private
`work/free-checkout-e2e-2026-11-19` directory. No credentials or provider payment
links are included in this report. No existing website work or application source
was removed or changed by the test.
