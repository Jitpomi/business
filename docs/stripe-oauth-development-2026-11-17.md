# Stripe OAuth development configuration — 2026-11-17

The existing JITPOMI Stripe sandbox OAuth application is configured for the account
website. Business uses the exact callback
`https://jitpomi.com/account/connect-callback.html`. OAuth for Stripe Dashboard
accounts is enabled in that sandbox. The sandbox platform key, client ID and
callback are installed on `jitpomi-business-validation`; no secret is committed.
The Blueprint marks all three values `sync: false` so operators provision them
explicitly without generating or replacing credentials.

## Deployment and boundaries

Render deployment `dep-dbucvhec6qcc73dcmsbg` succeeded using the existing verified
`4ea92664997d1dd27254e04258219dc46fcc2332` application build with its updated
environment. Current main's later changes were documentation/CI only. This was
an environment deployment, not a schema or provider migration. The database stays
`business_native_dev_v5`, with recreation disabled. Existing encryption, external
secret-store and authentication settings were retained. The original 20 service
environment keys remain; only these were added:

- `STRIPE_CONNECT_CLIENT_ID`: JITPOMI sandbox app registration.
- `STRIPE_CONNECT_PLATFORM_KEY`: matching existing sandbox secret, kept in Render.
- `CONNECTION_OAUTH_REDIRECT_URI`: the exact HTTPS callback above.

The application now advertises configured Stripe OAuth. Each connection requires
an authenticated tenant owner, browser-bound handoff and native TypeDB claim and
finish policies. Account identity and environment come from Stripe's authenticated
exchange response. The platform key plus bound connected-account header are used
as documented by Stripe; deprecated OAuth bearer/refresh tokens are not adopted.
The key is encrypted outside TypeDB for the connection and never returned by the
catalog. No billing default or payment consent is changed by connecting.

## Acceptance

The deployed website completed Auth0 login, native tenant discovery, real Stripe
sandbox authorization, callback exchange, connection creation and plugin
verification. Exactly one verified connection appeared. Its Stripe account was
different from the platform account. Read-only provider queries found no invoices,
payment intents or payouts in it. A second provider handoff was cancelled and
consumed without creating another connection; a forged callback was rejected by
the browser/company binding. The native TypeDB suite passed foreign actor/tenant,
expiry, replay and revocation before/during exchange checks. Live database probes
also denied foreign owner scope, hid the connection in the foreign tenant and
rejected decryption under that tenant's vault context.

A fresh v5 export was restore-verified before installation. All 1,750 objects and
attributes were unchanged immediately after deployment. Hosted acceptance retained
all objects: 1,749 original records stayed exactly unchanged, while the isolated
test tenant's serialization revision advanced through its intended writes.
The normal fence is implemented in `storage::write`; no broad revision exception
was introduced into the application or migration machinery.

The synthetic connection was disconnected locally, and its provider grant was
revoked using Stripe's supported deauthorization endpoint. Provider API access
then returned 403. The test account, audit records and encrypted connection were
retained. No payment, invoice status override, live-mode account configuration or
customer consent change was made. Original source backups and private validation
artifacts are retained outside source control.

The long acceptance session also exposed an expired-session logout issue in the
website. The website now clears an absent/expired/invalid local session
idempotently while retaining Origin checks; active revocation failures remain
explicit. Authenticated 401 responses leave the old document without retrying a
write. The website's 47 tests pass. See its
[account acceptance record](https://github.com/Jitpomi/website/blob/main/portal/docs/stripe-oauth-2026-11-17.md).

## Operations

Keep the app's callback registration and the service callback exactly aligned.
For another deployment, register its own exact callback and provision its own
mode-matched client ID/key. No arbitrary callback, tenant-provided OAuth endpoint
or automatic exchange retry is allowed. A lost exchange outcome requires a fresh
handoff after reviewing Connections; replaying a Stripe code can revoke its grant.

This acceptance is sandbox-only. Production requires a separately approved live
registration/key and live merchant acceptance. Stripe recommends Connect
Onboarding for new platforms; the existing-account OAuth capability here follows
its still-supported protocol. Other provider protocols remain plugin-owned.
Disabling OAuth configuration stops new handoffs but does not revoke existing
Stripe grants. Local Disconnect also does not perform provider revocation; that
boundary is explained on screen. Rollback must retain the encryption key,
external store, database, and connection/journal history.

Official references:
- https://docs.stripe.com/connect/oauth-reference
- https://docs.stripe.com/connect/oauth-standard-accounts
- https://docs.stripe.com/connect/testing?accounts-namespace=v1
