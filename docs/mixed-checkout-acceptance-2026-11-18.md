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
