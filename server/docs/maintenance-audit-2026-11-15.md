# Backend maintenance audit

Reviewed application composition, authentication/transport parameters, data and
portal services, provider registration, billing/storage, schema references,
operator tools, deployment configuration and test/CI wiring. This is a source
and local-test audit; deployed state was not inspected.

## Cleanup

Removed no-op startup modules, duplicate startup validation and unread builder
settings in the first pass. Moved the test-only Tower dependency to development
dependencies. This pass removes unread inner request headers/method/path/raw-query
fields and the unused Render ENVIRONMENT setting. Authentication retains outer
AuthParams headers, provider filters retain inner query, and credential guards
retain both provider markers. Historical verification notes are preserved in
verification-2026-10-20.md. CI now includes existing scheduler tests and runs when
the scheduler changes.

## Useful work retained

- The additive migration preserves identities and invoice history, pauses imported
  plans and keeps historical invoices from becoming new billing work. Its removal
  requires confirming migration and recovery needs across existing targets.
- The account-link installer updates an existing canonical sandbox without a
  destructive rebuild. New databases include the schema during provisioning.
- The external scheduler wakes sleeping hosts; the in-process loop cannot replace
  that function. Admission and database claims protect overlapping invocations.
- User-facing provider proxies and merchant-owned billing execution have different
  authorization and lifecycle responsibilities.
- Consent revisions, payment-method tombstones, durable claims and unknown-outcome
  handling protect against repeated charges and stale authorization.
- TypeQL assets are frontend examples, not a query authorization registry. Schema
  functions can be useful to operators without a direct Rust reference.
- External integration tests are explicitly gated rather than dormant tests.

No Rust source or test file was found outside the active Cargo target/module
graph. No public function name appeared only at its declaration in the source/test
reference scan. These checks do not prove that every exported API or database
function is used externally; uncertain candidates were retained.

## Validation

40 Rust tests and five JavaScript tests passed. Seven external integration tests
were intentionally ignored. Formatting, all-target build, Clippy with warnings
denied and documentation tests passed. No migration or live payment was run.

## Business flow follow-up

Traced account discovery, invitations/recovery, billing permissions, consent,
scheduling, issuance, collection, observations and notifications, together with
the companion website's Worker contract.

Removed the unused permission-fragment observation entry point and provider-ready
wrapper. Notifications now use one provider-independent rule directly. Account
recovery and ordinary API ingress share the same gateway comparison and the
BUSINESS_GATEWAY_SECRET configuration; the old PORTAL_ORIGIN_SECRET fallback is
not accepted by Business. The website can still name its Worker-side secret
PORTAL_ORIGIN_SECRET, but sends it under the canonical x-gateway-secret header.

Account operations and local scheduling no longer require a writable payment
connection. Missing providers still fail before an invoice issuance claim, and
payment/provider calls still require their configured credentials and permissions.
Removed a BILLING_PUBLIC_URL check whose value never affected the fixed JITPOMI
checkout destination. History now parses the provider enum strictly and checks
that its invoice binding exists rather than silently treating unknown providers
as Mercury or indexing a missing binding.

Added a separate real-database account/invoice test with no provider or email
calls. It covers invitation renewal/acceptance/replay/expiry, existing-account
protection, reset races, session revocation, canonical gateway enforcement,
one-time schedule completion, exact due dates, duplicate occurrence prevention,
provider-unavailable queue preservation, amount binding and stale observations.

The four existing real TypeDB backend tests passed, including policy enforcement,
provisioning through actual executables, concurrent reviews and HTTP rollback.
The new isolated account/invoice test also passed. Disposable databases used by
these tests were removed. Local Rust tests, formatting and Clippy passed.
Fresh real Stripe/Mercury payment and email acceptance was not run by this audit.

Companion website changes remove the retired backend switch and stop proxying
the removed payment-demo API. The old demo and tests are preserved outside the
Worker asset tree; its old URL redirects to the invoice workspace. The website
changes belong to the existing customer-account-portal PR, not this backend repo.
