# Automation backend validation — November 15, 2026

The backend now has a provider-neutral durable automation engine and tenant-owned
encrypted connections. Billing remains the authoritative domain implementation;
its consent, provider bindings, claims, invoice state and notification rules are
reused by workflow actions. No existing development or production database was
migrated and no service was deployed for this refactor.

## Checks completed

- Formatting, all-target build and ordinary Rust tests, documentation tests and
  Clippy with warnings denied. Frontend and health-wake worker Node tests passed.
- Real TypeDB automation acceptance, using generated disposable databases:
  encrypted secret handling, tenant-qualified records and pagination, immutable
  workflow history and stale edit rejection, customer data kept literal, event
  replay protection, approvals, worker restart/polling, disconnection, owner
  revocation, concurrent action claims, uncertain writes requiring review,
  authenticated owner API, and generic data read/write denial.
- Actual backup and automation installer/import executables against a disposable
  pre-automation model. The installer rejected an unconfirmed writer stop and a
  stale backup, preserved original records, and accepted repeated schema install.
  Import added encrypted connection/configuration records and billing workflow/
  schedule records without performing payment or mail sends.
- Two tenants completed billing through automation and independently resolved
  third-party-style bank/mail capabilities. Paid and declined outcomes produced
  the right receipt/reminder; subsequent runs and individual reconciliation,
  collection and notification steps did not repeat charges or deliveries.
- Existing real TypeDB domain policy, concurrent review and schema-policy HTTP
  tests passed. The actual provisioning/server test passed using its separate
  non-admin runtime credential file.
- Account/invoice acceptance without external providers passed through the new
  dynamic billing ingress, including invitation renewal, acceptance/replay,
  tenant permissions, recovery, password reset races and session revocation.

All test database changes were confined to generated `business_backend_test_...`
targets, which the tests deleted. New Stripe/Mercury transactions and real email
sends were not part of these checks. The separately gated real-provider tests and
previous settlement evidence remain available; these results do not certify a
new live payment flow or inbox delivery.

## Deployment boundary

Follow `server/AUTOMATION.md` before deployment: stop writers and the old scheduler,
export and restore-verify a fresh private backup, install the additive schema,
import each existing tenant while retaining provider IDs, configure the vault key,
then enable the new runtime. Auto-deployment remains off. Startup refuses to run
billing without automation or automation without its schema.

The implementation is an API and worker foundation. A visual builder, OAuth
refresh, uploaded code execution and in-place credential/master-key rotation are
not implemented. Connections and published versions are deliberately immutable;
there is no silent replacement of an account behind existing invoice bindings.
