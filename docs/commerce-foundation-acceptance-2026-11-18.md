# Commerce foundation acceptance — 2026-11-18

This is data-foundation acceptance, not a checkout release or payment certification.
The agreed scope and remaining delivery work are in [commerce-and-checkout.md](commerce-and-checkout.md).

Implemented: immutable merchant offers and published pricing policies, buyer order
agreements with immutable lines, fixed/custom donation bounds, recurring terms,
merchant-approved finite instalment/deposit terms, native integer totals and due
amounts, historical-order visibility and scoped catalogue/order query assets.
The existing /data schema-policy adapter executes the native policies; no new
commerce service, credentials, provider calls or background scheduler is introduced.

Validation completed:

- Real development TypeDB, TLS, generated UUID database, using the production
  SchemaPolicyAdapter: `native_commerce_pricing_isolation_and_immutable_orders`
  passed in 105.28 seconds. The successful fixture was deleted after verification.
- Accepted purchase, recurring service, one-time donation, recurring donation,
  finite instalments, deposit plan and multi-line cart agreements.
- Rejected merchant offer creation by a buyer, changed fixed prices, out-of-bounds
  donations, multiplied donation quantities, unapproved recurrence/cadence/count,
  undersized instalments, invalid deposits, foreign-tenant orders, duplicate IDs,
  changed accepted terms, late cart lines and revoked buyer access.
- Verified that publication withdrawal blocks new orders without hiding historical
  accepted terms; an outsider cannot read orders. Both buyer and merchant can
  review native totals and payment amounts through scoped context functions.
- Verified instalment rounding: 1001 minor units becomes 333/333/335; with a 300
  deposit, 300/350/351. No fourth obligation exists. Two identical cart lines retain
  their separate contributions: subtotal 6006, discount 600, shipping 100, tax 275,
  total 5781.
- Default Rust suite: 57 passed, 44 explicitly gated integrations ignored.
- Clippy with `-D warnings`, rustfmt and diff whitespace checks passed.
- Existing website account suite: 47 passed. The website was not modified.
- Existing frontend data-client tests passed.

Existing development data was not migrated, reset or updated. Existing invoice,
payment, receipt, reminder, authentication and queue execution code was unchanged.
No provider payment or email was sent. No checkout UI was deployed. Real payments
remain disabled; dependency-maintenance findings were not changed or suppressed.

Remaining before checkout can be offered: durable obligations and native claims,
tenant-plugin capabilities and execution, consent integration, safe hosted/embedded
onboarding, merchant/buyer screens, refund execution, tax/shipping quotes and
fulfilment/reservations, then preservation-checked migration, recovery and full
provider acceptance. The initial schema supports USD and authenticated existing
buyer accounts; guest and multicurrency checkout are not implemented.
