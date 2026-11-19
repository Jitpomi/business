# Mixed-cadence checkout acceptance

One native checkout seals a set of independently priced orders for one customer,
currency and published pricing policy. Every schedule is accepted atomically;
there is no partial signature path through the existing single-order endpoint.
Provider invoices and consent remain per order. The first payments are separate
provider invoices, not a consolidated charge. The review shows each schedule’s
shipping, discount, tax, first payment and recurring terms.

## Verified boundaries

- Native policy tests passed on TypeDB 3.14.1 and 3.15.0: buyer/merchant reads,
  foreign-tenant isolation, seller acceptance denial, incomplete acceptance,
  single-order bypass denial, quote group counts, currency mismatch, duplicate
  cadence groups and late membership changes. Existing price, instalment,
  currency, method restrictions and immutable-order checks remain included.
- The actual website gateway, dog-auth, TypeDB 3.14.1 and dog-queue passed the
  dedicated mixed-cart flow in 186.61 seconds with independent synthetic payment
  and notification plugins. It verified three manual invoices, paid/processing/
  open states, no implicit collection consent, one settled-payment receipt,
  partial refund, lost-acknowledgement read recovery, cancellation and no duplicate
  provider invoice, refund or notification calls.
- The combined gateway regression passed on TypeDB 3.15.0 in 548.40 seconds. It
  additionally covers the existing one-time purchase, wrong-currency settlement
  denial, processing without receipt, refund recovery, finite instalments with
  exact rounding, recurring donations, separate consent grants, unapproved manual
  subscriptions and durable-worker restart.
- All-target Rust tests, strict Clippy and 56 website tests passed. The website
  contract tests check one native quote write and one complete acceptance write,
  with idempotent recovery and no direct provider or billing-plan creation.
- The hosted layout was visually reviewed using clearly synthetic local fixtures
  with one-time, monthly and yearly JPY amounts and different payment statuses.
  Visual fixtures are not financial or authorization evidence.
- The downloaded encrypted v7 checkpoint was rehearsed on isolated TypeDB 3.14.1.
  All 3,112 original objects, attributes, relation roles and existing plan-approval
  decisions were preserved. Migration preparation and rehearsal do not write the
  application cluster; only a verified candidate is imported during explicit
  cutover, followed by another complete graph and approval check.

## Deployment boundary

This branch targets `business_native_dev_v8` through an explicit development
shadow migration. Startup refuses
an older schema. No runtime schema migration, credential rotation, new consent,
provider effect or payment-state override is performed by the migration. Source
databases, checkpoints and recovery shadows are retained. Real checkout payments
remain disabled. Inline card widgets and Mercury refund execution are separate
capabilities and are not claimed by this change.

## Explicit retirement of the old development copy

After the successor exactly matches the quiesced checkpoint, `migrate_checkout
--retire SOURCE TARGET CHECKPOINT` records a private intent and suspends tenants
only in the retired database copy. The successor retains every original tenant
state. Existing native membership, billing and automation policies then deny
application access in the old copy, preventing Render’s resume step from running
old workflows against it. Authentication credentials, memberships, consent,
invoices, payments, refunds and secret references are not rewritten.

The command requires stopped writers, consecutive development generations, an
unchanged source and an exact verified successor. It checks every source record
and every relationship after the intentional tenant-state change, and checks
that the successor remains identical to the checkpoint. An intent journal permits
read-after-unknown-acknowledgement recovery without blindly repeating a write.
It has no production target or automatic startup path.

A loopback-only test using the complete checkpoint proved that an unverified
successor prevents retirement, native reads and writes are denied in the retired
copy, successor access remains valid, a repeated retirement is safe, and restoring
only the recorded original active tenant set restores the complete original
graph. No worker or provider ran on either fixture. Rollback in development must
first stop and fence the successor before restoring access to the old copy; two
active generations must never be started together.

## Development cutover completed

Business #40 was merged as `edce33f` and deployed to the development service on
`business_native_dev_v8`. Website #11 was merged and the account/checkout Worker
was deployed as `5993b5b2-faf8-4b6c-b698-29071314420b`.

Writers were stopped before the fresh checkpoint. Encrypted offsite backup run
44434662579 passed independent recovery. Its downloaded/decrypted manifest,
all 3,502 original object values and complete graph matched the stopped source.
Migration checked the old and new schemas locally before importing the verified
candidate. Its final full graph and plan-approval checks passed. Explicit source
retirement suspended only the old copy's 48 active tenants; successor tenant
states, credentials, consent, financial records and relationship roles remained
unchanged. Source databases and private recovery evidence were retained.

The v8 backup run 44435477326 passed independent isolated recovery. Its retrieved
archive was decrypted and manifest-verified: all 3,502 original objects and
relationships matched the stopped checkpoint, with 22 ciphertext rows and 18
connection references. Backup bindings now target v8.

A fresh hosted Auth0 sign-in loaded the deployed catalogue and original order.
Native buyer reads included the original invoice and excluded a foreign invoice;
a buyer refund proposal was denied without creating a record. Read-only Stripe
and native checks matched the original paid USD 1.00 invoice, succeeded USD 0.40
refund and original reminder/payment/refund receipt identifiers. No financial
journal was replayed.

One startup commerce-cycle action entered attention. Its exact attempt, single
finished one-time plan, paid invoice, succeeded refund, and a later independently
completed identical cycle with zero errors were verified before cancelling only
that obsolete run with evidence. The original attempt and error were retained;
no provider was called and no payment status was overridden. Final readiness
reported all seven counters at zero. Merged correctness passed; the separate
strict dependency audit still reports the same three upstream TypeDB findings.
Real payments remain disabled.
