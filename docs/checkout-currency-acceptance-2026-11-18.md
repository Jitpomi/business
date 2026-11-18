# Checkout currency acceptance — 18 November 2026

This extends the sandbox checkout to USD, EUR, GBP, CAD, AUD and JPY. It does not
perform foreign exchange, combine currencies in one order, change the
matching-cadence limitation, embed a card form, or add unsupported Mercury refunds.
Real payments remain disabled.

## Evidence

- Real TypeDB 3.15 scoped pricing tests: all six currencies, immutable currency,
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
