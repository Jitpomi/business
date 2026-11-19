# Checkout currency acceptance — 18 November 2026

This extends the sandbox checkout to USD, EUR, GBP, CAD, AUD and JPY. It does not
perform foreign exchange, combine currencies in one order, change the
matching-cadence limitation, embed a card form, or add unsupported Mercury refunds.
Real payments remain disabled.

## Evidence

- Real TypeDB 3.15 and isolated 3.14.1 scoped pricing tests: all six currencies, immutable currency,
  rejection of cross-currency offers and USD pricing applied to foreign currency,
  and rejection of a USD-only saved method for a JPY plan.
- Actual website Worker, dog-auth, TypeDB and dog-queue acceptance in JPY: cart,
  agreement, scheduled invoice, wrong-currency paid observation rejected,
  processing without a receipt, settlement with one receipt, seller-only partial
  refund, lost-acknowledgement read-only recovery, independent order consent,
  finite instalment rounding, recurring donation and restart deduplication.
- Stripe TEST EUR and JPY contracts: each created a synthetic 1,000-minor-unit
  invoice, settled one manual test-card payment, refunded 200 minor units once,
  recovered the refund read-only and verified currency-bound payment history.
  Journals precede uncertain external effects; rerunning an existing test root is
  rejected. No real-money key is accepted by this test.
- Existing development checkpoint rehearsal: all 1,967 original objects,
  attributes, relation roles and existing plan-approval decisions preserved.
  Only new isolated databases were used; the source was untouched.
- Website tests include exact zero-decimal entry, excess precision/overflow
  rejection, currency-bearing native contracts and private API isolation.

## Migration and deployment

This schema change requires the explicit checkout shadow migration before the
new runtime starts. Its startup guard rejects the older schema. Keep the original
source database and encrypted checkpoint. Refresh the scheduled-backup database
configuration and sealed recovery capsule only as part of the development cutover.
Deploy the backend before the new frontend query contracts. Do not enable real
payments as part of this change.

Historical pricing without a currency and non-commerce billing plans remain USD.
Existing external IDs, invoice links, consent grants and delivery journals are
preserved. Existing field names ending in `_cents` remain compatibility labels for
integer minor units and now include a currency in their read projections.

## Hosted validation interruption

The hosted 3.14.1 end-to-end attempt was interrupted by a database transport
failure and queue shutdown errors while the cluster restarted. A second native
attempt also lost its gRPC stream before commit. Both generated test databases
were retained; no ambiguous provider operation was replayed. The restart’s root
cause has not been established, so this is not a passing hosted acceptance run.

The unchanged deployed backend temporarily returned dependency-unavailable and
then recovered to HTTP 200 with all seven operational counters clear. Read-only
inspection confirmed its existing settled payment, successful partial refund and
original payment/refund receipt IDs were unchanged. This feature remains in its
pull requests, pending a controlled development migration and deployment.

## Sequential hosted verification

The follow-up run passed on the hosted development TypeDB 3.14.1 cluster in
542.33 seconds, using a generated isolated database and no simultaneous schema
or restore test. It exercised the actual Worker, dog-auth, native policies and
dog-queue, including JPY settlement, wrong-currency rejection, one partial refund,
read-only recovery after a lost acknowledgement, independent consent, finite
instalments, recurring donations and restart deduplication. The successful test
removed only its generated fixture database. It did not migrate the deployed
application database or enable real payments.

The deployment target for the explicit v6-to-v7 cutover is
`business_native_dev_v7`. Keep the source database and fresh encrypted checkpoint
before switching the runtime and scheduled backup configuration.

## Development cutover and offsite recovery

Business #38 and Website #10 were merged. Development runs Business `3f973ef`
on `business_native_dev_v7`, with website `0bd5b8d` and Worker version
`ab78edd9-e400-4cb2-9a88-eb1a120fa557`. The quiesced checkpoint contained 2,947
objects; the shadow migration verified every original attribute, relationship
role and plan approval. The source database and recovery evidence remain retained.
Forty-two financial objects were independently compared without attribute changes.
The existing hosted order survived fresh Auth0 sign-in and still showed the
original paid invoice and successful partial refund. Read-only Stripe/native
checks matched the original reminder, payment receipt and refund receipt IDs.
No payment or refund request was replayed. Real checkout payments remain disabled.

A TypeDB restart interrupted the first deployment and cloud restore rehearsal.
Its exact cause is not established. Resume-period changes were reviewed and
retained in the old database and private checkpoints; they contained scheduler
and capability-refresh activity, with no changed invoice, payment, refund or
receipt. Ten obsolete interrupted billing-cycle attempts were individually
reviewed against later successful cycles and unchanged financial facts, then
cancelled through the native owner-scoped evidence path. Attempts and errors
were retained. A known rolled-back storage-conflict cancellation was retried
without invoking a provider. All seven operational counters subsequently cleared.

Business #39 moved both manifest imports and independent restore rehearsals off
the application cluster to an isolated, digest-pinned TypeDB runner container.
The source connection only exports transactionally; cloud TLS, native policies,
encryption, alerts and upload acknowledgement remain enforced. The runner test
and the fresh main backup run `44424035847` passed. Downloaded recipient-encrypted
archives were decrypted and manifest-verified. An isolated full restore and
ciphertext-store drill verified 22 ciphertext rows and 18 tenant connection
credentials, rejecting the wrong key and wrong tenant without contacting the
source secret store. The existing separately held recovery identity was retained.
