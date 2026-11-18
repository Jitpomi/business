# Shared commerce and checkout

The agreed product scope covers products, services, carts, invoice payments,
one-time and recurring donations, subscriptions, instalments, deposits and
remaining balances, discounts, taxes, shipping, refunds, receipts, hosted checkout
and embeddable widgets. These are one commerce system, not separate payment apps.

## Current implementation boundary

The shared checkout includes a merchant catalogue and immutable pricing versions,
native quotes, confirmed orders, hosted checkout and an embeddable button/cart/
donation/plan widget. Confirmed orders become ordinary billing plans once; the
existing invoice, reconciliation, receipt and reminder lifecycle remains the
payment authority. Existing invoices reuse their original provider payment link.

One-time purchases, matching-cadence subscriptions and recurring donations, fixed
or chosen donations, finite instalments and deposits share that lifecycle.
Merchant-approved flat discounts, tax rates and shipping amounts are included in
the native quote snapshot. These are not a jurisdiction tax service, stock system
or fulfilment integration. Mixed one-time and recurring carts must be checked out
separately. Checkout supports USD, EUR, GBP, CAD, AUD and JPY. Each order and its pricing policy use one currency; there is no implicit conversion. Existing invoice minimums still apply in minor units, and the provider may enforce additional account/settlement minimums.

Checkout execution remains sandbox-only at the server, scheduler and capability
boundaries. A browser setting cannot enable real payments. The widget hands off to
our hosted checkout for account authentication and native price review; it does
not embed a card form or expose credentials. New customers may enrol only where
the merchant explicitly permits public checkout; existing private billing is not
made public. A signed, verified provider identity receives an independent buyer
account, never merchant membership and never an email-based identity merge.

Automatic collection requires an immutable approval for the reviewed order's
plans, verified selected method and current customer revision. Separate orders
can have independent approvals at that revision. Grant plan counts seal the scope
against later extension. Historical grants remain unchanged and read-valid;
changing the selected method or revoking the revision invalidates old approval.
Saving a method alone authorizes no charge. Existing manual invoices stay manual.

Refund requests are authorized by the seller in TypeDB and executed by the
merchant's own financial plugin through the scheduled dog-queue billing workflow.
Stripe supports partial refunds for a single settled PaymentIntent invoice in the order’s currency.
Mercury does not declare refund capability. Unsupported payment allocations fail
before a provider write. Cumulative nonfailed refund reservations cannot exceed
the invoice amount. A lost acknowledgement is reconciled read-only by the
immutable request ID; it is never resubmitted. Success produces a separate durable
refund receipt. Cancellation stops future obligations without erasing invoices
or inventing a refund. External refunds made outside this app are visible through
provider history but are not automatically imported as commerce requests.

Explicit migration imports a verified development checkpoint into a new database
and compares every original logical object, attribute and relationship. It rejects
unreviewed custom definitions, keeps the source and recovery shadows, never
creates consent and never runs providers. No startup schema migrations occur.

## Ownership and boundaries

TypeDB owns merchant and buyer relationships, published offer revisions, price
and donation bounds, quote inputs, order terms, consent, balances, finite payment
obligations, reservations, permitted transitions and multitenant RBAC. The normal
`/data` read/write capabilities execute frontend TypeQL under dog-auth's verified
identity, tenant and server time. A new commerce service that reimplements these
rules in Rust is unnecessary.

Prices, tax/discount/shipping policy revisions and accepted orders are immutable.
Publication may be withdrawn, but the buyer and merchant can still read the terms
of their accepted orders. New prices require new offer revisions. An accepted
order cannot acquire extra lines later. Cart editing precedes acceptance; changing
accepted terms requires a new review and agreement, not silently updating a price.

Buyers cannot select arbitrary instalment terms: a published merchant policy
controls whether instalments are offered, allowed cadence, maximum count and
minimum deposit. Integer minor units, bounded quantities and a bounded total avoid
floating-point pricing. The last instalment receives the rounding remainder;
there is no extra payment after the finite schedule. A recurring donation's chosen
amount becomes part of its accepted terms, not a new choice on every collection.

A merchant pricing policy can specify a discount, shipping amount and tax rate.
This is a published quote policy, not jurisdiction detection. Before automatic tax
or address-based shipping is offered, dedicated plugins must supply a versioned,
expiring quote with the relevant address and goods classification, and TypeDB must
bind that quote to the reviewed cart. The browser never supplies trusted tax,
shipping or discount amounts. Restricted coupons need explicit eligibility,
redemption limits and atomic use accounting; public pricing policies are not
private coupon codes.

The first native model uses existing authenticated buyer accounts. Guest checkout
needs its own scoped checkout grant and identity/account onboarding. It must not
create tenant membership merely because a visitor knows an invoice or offer ID.

## Plugin capabilities

Each tenant connection must declare supported currencies, minor-unit precision,
hosted or embedded presentation, payment methods, idempotent creation, read-after-
write recovery, settlement observation, refunds and off-session collection. Missing
capabilities fail with a clear unsupported-operation response before an external
write. Browser input cannot choose another tenant's connection or supply secrets.

Commerce recurrence and instalment obligations are independent of provider-native
subscription products. dog-queue executes scheduled obligations selected and
claimed in TypeDB. A plugin may collect only after a current, scope-specific consent
grant authorizes the amount, currency, cadence and payment method. Saving a method
is not consent. Cancellation/revocation blocks future claims; it does not invent a
refund or reverse a settled historical payment.

Invoice checkout reuses the original invoice and hosted payment link. It must not
create an unrelated Stripe Checkout session that can collect the same debt twice.
A cart purchase first becomes one durable order; payment obligations derive from
that order, and provider objects are bound to its stable operation identifiers.
An ambiguous non-idempotent result requires reconciliation, never automatic replay.

## Payment and order lifecycle

An order agreement, a payment attempt, settlement and fulfilment are distinct facts.
A provider return URL, popup message or webhook delivery alone never marks an order
paid. Verified observations must match tenant connection, external reference,
customer, currency and amount. A bank transfer remains processing until settlement
is observed. Refunds have their own idempotency keys and provider-confirmed facts;
requesting a refund does not establish that money was returned.

One-time purchases, subscriptions and finite plans share the same obligations,
attempts and observation model. Mixed carts must explicitly separate one-time and
recurring obligations and display the amount due now and each future cadence.
Subscription cancellation, plan completion and fulfilment are separate states.
Tax/discount/shipping snapshots and partial refunds retain enough line allocation
to reconcile the original charge. Receipt and reminder delivery use the tenant's
notification plugins and existing durable deduplication, with precisely one receipt
per settled payment. Refund acknowledgements are separate notifications.

Stock and limited-service capacity require native reservations with expiries,
atomic acceptance and dog-queue expiry processing. A paid order cannot imply that
shipping or service delivery is complete. Physical shipping additionally needs
address handling, merchant shipping rules and fulfilment/tracking records.

## Hosted pages and embeds

The hosted checkout and widget share the same native query contracts and review.
A tenant can embed a cart, buy button, donation form or payment-plan selector. The
snippet contains public merchant/offer identifiers, never a provider secret or
trusted amount. It must display the merchant, total and future obligations clearly.

Public SDK assets must live outside the existing Cloudflare Access-protected
`/account` route. Keep that protection and the encrypted HttpOnly SameSite session
cookies intact. Use a top-level authenticated handoff for providers or browsers
that cannot support embedded payment; do not depend on third-party cookies or
weaken cookie attributes. Any popup integration checks the exact origin, source
window and one-time state. Messages carry no credential and are not proof of
payment. Returning from hosted authentication preserves only validated relative
checkout context, never an arbitrary redirect URL.

Keep accessible keyboard navigation, clear pending states, a visible hosted-page
fallback, #007aff and ordinary language. Payment details belong to the selected
provider's supported collection experience, not a custom card-number form.

## Delivery and acceptance

1. Native catalogue/order/pricing foundation: adversarial scoped reads and writes,
   price tampering, donation bounds, tenant ownership, withdrawn offers, immutable
   terms, multiple identical cart lines, finite-plan rounding and revocation.
2. Obligations and payment execution: native atomic order acceptance and claims;
   dog-queue scheduling; tenant-plugin capabilities; uncertain-write recovery;
   order-to-existing-invoice binding and exactly-once application notifications.
3. Checkout UX: merchant catalogue/quote management, buyer carts/review, safe hosted
   authentication, explicit recurring/plan consent and public widget handoff.
4. Full commerce: coupon redemption, tax/shipping quotes, reservations, fulfilment,
   cancellation, partial refunds and mixed recurring carts with explicit allocations.
5. Isolated development migration: fresh encrypted checkpoint, additive shadow
   import, full graph preservation, runtime policy checks, recovery rehearsal,
   controlled cutover and backup configuration refresh before deployment.
6. Sandbox acceptance through independent tenant connections: one-time/cart,
   invoice reuse, donations, recurrence, deposits/instalments, refunds and receipts;
   revoked access, duplicate callbacks, concurrent checkout, restart and delayed
   settlement. Do not report an unavailable provider capability or a pending bank
   settlement as a passing money-flow test.

Production remains disabled until the whole deployed path, monitoring, backups,
recovery and provider production onboarding pass their separate acceptance gates.

## Currency and saved-method capabilities

Currency is immutable on offers and orders, and explicit on new pricing versions.
Historical pricing without a currency and non-commerce billing plans retain their
original USD meaning. Invoice and refund currency is derived from the immutable
order, never from a mutable customer preference or browser label. Native checks
reject cross-currency lines, pricing, payment observations and refund observations.

Connections declare supported presentment currencies. Server-owned attestations
replace the full set atomically with a freshness timestamp; they contain no
credentials. Stripe declares the six admitted currencies above; Mercury declares
USD. Acceptance and background execution require a fresh matching attestation.
Additional provider account and payment-method restrictions can still apply.

A saved payment method declares its own currency set. Stripe cards support the
connection’s admitted currencies; US bank accounts support USD. Unknown third-party
methods conservatively default to USD until their plugin explicitly declares
otherwise. TypeDB checks every plan in a consent grant against that method’s set;
collection rechecks it. No existing consent grant is expanded during migration.

JPY has zero decimal places; other admitted currencies have two. All stored and
transmitted amounts remain integer minor units. Entry rejects fractional yen and
excess decimal precision. Notifications, order review, dashboard history and
refund entry display the actual currency. Existing `amount_cents` API field names
remain compatibility labels for integer minor units; consumers must use the
accompanying `currency` rather than assume a divisor of 100.
