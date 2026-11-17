# Tenant-owned connection onboarding

`Connector::oauth()` is an optional installed provider adapter. It owns the
provider protocol and returns a validated account binding plus private
credentials. The automation engine does not know which financial or notification
provider is selected. `connection_fields`, `connection_purpose` and `internal`
provide the UI catalog; they do not grant access or change billing policy.

`oauth_begin` creates a ten-minute durable handoff. The random state is stored as
a digest, and its private verifier is encrypted in the existing external vault.
TypeDB receives only its opaque reference and non-secret binding metadata.
`oauth_finish` uses native `automation_oauth_claim` and
`automation_oauth_finish` functions. These enforce the initiating identity,
tenant, owner membership, expiry and one-time transition. Completion atomically
inserts an unverified connection and completes its handoff. Provider exchanges
are never retried after an uncertain result. No browser-supplied provider URL,
redirect, account ID or permission is trusted.

The existing verify, disconnect, workflow and payment paths remain authoritative.
An OAuth-linked account needs connector verification before it can run work.
Disconnect retains its record and blocks new use; it does not silently revoke a
provider grant or change customer payment consent. Credentials never enter
TypeDB. Account configuration reads no longer instantiate payment plugins or
open their credentials, allowing an owner to repair a broken connection while
financial execution continues to fail closed.

## Provider setup

Stripe Connect is the first OAuth adapter. Configure the matching development
`STRIPE_CONNECT_CLIENT_ID`, `STRIPE_CONNECT_PLATFORM_KEY` and
`CONNECTION_OAUTH_REDIRECT_URI` outside source control. The callback must be an
exact registered HTTPS URL ending `/account/connect-callback.html`. The website
is https://jitpomi.com/account/connect-callback.html. No provider registration is
created by startup, and absent configuration is visibly unavailable in the UI.

Stripe uses its documented confidential Connect code flow, then the platform key
with the account ID authenticated by Stripe. Deprecated bearer/refresh tokens
are not used. The shared provider contract supplies a verifier for adapters
supporting PKCE; it does not pretend Stripe's documented protocol supports it.
Additional providers must implement their own grant, refresh/expiry and
revocation semantics before advertising OAuth. Mercury, Resend and generic HTTPS
API-key onboarding remain available.

Protocol sources:
https://docs.stripe.com/connect/oauth-reference
https://docs.stripe.com/connect/oauth-standard-accounts
https://docs.mercury.com/docs/integrations-with-oauth2

## Development rollout

Runtime verification requires both new native functions; startup never migrates
or resets a database. Use `database_backup export` and `restore-verify` with a
private backup directory. Rehearse with:

```
BUSINESS_ENV_FILE=<private-development-env> cargo +1.98.0 run --bin migrate_business_rules -- --rehearse business_native_dev_v4 <private-backup-directory>
```

The generic development shadow migration preserves the exact exported schema,
records, relation-role bindings, queue jobs and custom work, adding only missing
native policy definitions/facts. It rejects production, downgrade, stale backup
and existing-target cutovers. The reserved development namespace is strictly
validated. `--apply` requires `BUSINESS_WRITERS_STOPPED=true`, compares the fresh
source with its backup, and derives the next versioned shadow. Keep the original
source and verification shadows. Deploy the verified backend against the new
shadow before deploying the account Worker. Retain the vault key and secret
store; configure OAuth registration only for the intended environment.

## Verification

The gated `oauth_is_native_tenant_actor_expiry_and_replay_fenced` test uses a
synthetic provider and a real disposable TypeDB database. It covers another
tenant, a different owner, expiry, denial, replay, concurrent callbacks, owner
revocation before and during exchange, verification and secret separation.
It also proves a disconnected default provider cannot block safe account
configuration reads, while financial resolution still fails closed.
The existing native-policy and independent tenant billing suites remain in use.
The website suite checks browser-bound handoffs, company switching, CSRF and
callback code removal. These are not a claim of live-provider OAuth acceptance;
that requires a configured provider app and its hosted authorization flow.
