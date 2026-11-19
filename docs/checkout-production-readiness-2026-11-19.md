# Checkout production readiness audit — 19 November 2026

**Real payments remain disabled. This is not a live-money release approval.**
The current deployment remains development validation. Production infrastructure,
live merchant registration and a separately authorized live acceptance remain
explicit gates; sandbox success does not prove real settlement.

## Audit coverage

| Boundary | Authority and verification |
| --- | --- |
| Identity and tenant access | dog-auth validates proofs; native TypeDB policies bind identities, active memberships, merchant/buyer access, scoped reads and writes. No credentials enter the graph. |
| Prices and cart acceptance | TypeDB owns immutable offers, currency, donation bounds, totals, instalment bounds, mixed schedules, signatures and consent. Frontend inputs are proposals. |
| External effects | Tenant-selected plugins bind invoice, customer, currency and amount. Native claims, immutable IDs and provider observations preserve payment/refund safety. Unknown outcomes are not blindly resubmitted. |
| Widgets and hosted pages | Exact approved origin, popup, nonce, merchant and invoice bindings; account proofs remain in HttpOnly cookies; provider fields remain provider-owned. |
| Scheduling and recovery | dog-queue drains durable occurrences. Native fences and journals survive restart. Backups include full graph, external ciphertext and separately encrypted runtime configuration. |
| Operations | Protected aggregate probes and independent alert/watchdog services; no monitor invokes a payment, tenant workflow or business-state write. |

## Gaps corrected by this audit

- Explicit TypeDB `STC2` isolation aborts now retry only their server-owned
  database transaction, at most three attempts within the original thirty-second
  deadline. All native predicates and fences execute again. String-matched errors,
  application denials, timeouts and unknown commit acknowledgements never replay.
  External provider calls remain outside this transaction boundary. A forced real
  server conflict verifies one aborted attempt and one committed resource.
- Receipt sends that were uncertain, or interrupted in `sending` for over fifteen
  minutes, were retained safely but invisible to readiness. The new
  `attention_notices` aggregate makes them alertable without resending them.
- Refund submissions stuck in `queued` or `processing` for over fifteen minutes
  now appear as `stalled_refunds`. Provider-confirmed `pending` refunds remain
  distinct; they can legitimately take time. Existing `attention_refunds` remains.
- Readiness protocol v2 requires both new aggregates. The monitor also understands
  v1 for an ordered rollout: deploy the observer first, then the backend. Missing
  v2 fields fail closed. Outage and recovery alerts retain their durable history.
- Inline payment fields now cancel a delayed mount when their widget is removed
  or a native settlement/status update arrives. Teardown during provider
  confirmation cannot overwrite the settled display or enable another submission.
- Payment return URLs require the exact JITPOMI HTTPS origin, including its port.
- Hosted sign-in reports temporary enrollment, verification or profile failures
  as unavailable, rather than claiming the buyer lost access. No session is
  granted by these error paths; encrypted retry context is preserved.
- Checkout errors no longer promise that an unacknowledged request changed no
  state. Buyers are directed to the existing order/payment record.

These changes introduce no schema migration, payment-state overrides, new
credentials, wider tenant access or automatic retry of an uncertain charge,
refund or customer receipt.

## Remaining release gates

1. The strict dependency audit still reports the existing Chrono, Chrono-TZ and
   Chrono-TZ-build maintenance findings. They are not suppressed. The upstream
   parser proposal remains draft; it is not an available adopted release.
2. Production capacity, availability and a production-specific least-privilege
   deployment have not been established by the free development service. No paid
   hosting purchase or production database migration is authorized by this audit.
3. Live Stripe platform/merchant registration and exact callback/webhook setup
   require a separate activation. Production recovery and key custody must be
   verified against that eventual deployment before live acceptance.
4. Production recovery objectives need an explicit decision. Daily encrypted
   archives and a tested restore do not establish zero data loss between snapshots.
   Set acceptable RPO/RTO and verify the production backup/journal arrangement
   before accepting real money.
5. Mercury refunds remain unavailable. Its bank-payment settlement is not claimed
   verified merely because sandbox payment initiation works.
6. The supported product boundary remains explicit: six currencies, versioned flat
   tax/discount/shipping policies, up to sixty cart lines and four schedules;
   separate first invoices for mixed schedules. Automated tax jurisdiction,
   fulfilment/stock and guest access are not implied by checkout success.

The live audit identified one retained uncertain periodic sandbox cycle from the
preceding deployment. Independent read-only Stripe and native checks confirmed its single invoice,
settled payment, succeeded partial refund and accepted notices. The owner-scoped
native cancellation recorded that evidence and verified later cycles already
complete. Its original attempt/error remains retained. No provider write or
financial status override was used.

## Provider contracts checked

- [TypeDB transaction conflicts](https://typedb.com/docs/core-concepts/drivers/transactions/):
  explicit aborted write transactions can be retried; unknown external outcomes
  require separate reconciliation.
- [Stripe payment lifecycle](https://docs.stripe.com/payments/payment-intents):
  reuse the invoice's existing payment intent; confirmation is distinct from native
  settlement and notification authority.
- [Stripe refunds](https://docs.stripe.com/refunds): pending refunds and completed
  refunds are distinct; no local status override substitutes for provider results.
- [Stripe webhooks](https://docs.stripe.com/webhooks): signatures, current provider
  observations, duplicate/out-of-order safety and independent reconciliation.
- [TypeQL proposal #477](https://github.com/typedb/typeql/pull/477): still draft on
  this audit date, with the broader driver work separate.

## Final development acceptance

- Business runtime `95414968d06caad89a2a889d09e9e7ba756f9b22` deployed
  successfully on 19 November. No schema, database generation, credentials or
  live-payment settings changed. Subsequent acceptance commits contain tests/docs.
- Correctness CI passed for the exact runtime source, including strict Clippy.
  The separate strict dependency workflow remains red for the same three findings.
- Full checkout gateway scenarios passed on real TypeDB 3.14.1 (488.41 seconds)
  and 3.15 (549.68 seconds): native pricing/currency/tenant boundaries, processing
  without premature receipts, exactly one settled receipt, ambiguous refund
  recovery without a second provider write, instalments, recurring donations,
  mixed schedules, partial refunds and cancellation through dog-queue and plugins.
- Forced native transaction conflicts passed on both versions. A concurrent owner
  revocation is rechecked on the fresh transaction and leaves no unauthorized
  resource; ambiguous errors and application denials never retry.
- Seventy frontend tests and eleven monitoring tests passed. The final widget fix
  aborts the removed view immediately while allowing another widget to finish
  using the shared provider SDK download.
- Account/checkout worker `fe329f0c-f8e9-491d-86ef-32271cdd601c` deployed from
  website main after PRs 14 and 15. Fresh hosted sign-in displays the retained
  paid sandbox invoice and succeeded USD 0.40 refund, with no second Pay button.
- The deployed native buyer scope returned its own invoice, filtered a foreign
  invoice, rejected a seller refund and left no denied proposal. Independent
  provider reads verified all three retained Stripe fixtures: one settled payment
  and one succeeded partial refund each, without new charges or refunds.
- Monitor `500839db-edbf-40d5-b543-9f4202a3ed76` reports all nine counters zero,
  no pending alerts and a fresh backup. A controlled monitor-only outage/recovery
  produced exactly one accepted message of each kind; both arrived in the
  development operations inbox. It invoked no customer payment workflow.
- Writers drained before the encrypted checkpoint in backup run `44538923625`.
  The independent recovery restored 9,025 graph records and 26 ciphertext rows,
  verified all 22 connection credential references without contacting the source
  secret store, and rejected wrong-key and wrong-tenant access. Original database
  generations and the isolated recovery shadow remain preserved. Daily 09:17 UTC
  encrypted backups with thirty-day artifact retention remain enabled.

Verification logs, provider observations and recovery evidence are retained in
`work/checkout-final-audit-2026-11-19` in the private operator workspace. No secrets
are included in this repository report. These results authorize continued sandbox
validation, not real-money activation.
