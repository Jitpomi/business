# Authentication plugins and the TypeDB identity graph

Authentication providers establish who is acting. The TypeDB data layer determines
what that identity may read or change. Application/proxy services do not maintain
another set of tenant roles, grants or RBAC rules.

The foundation follows the TypeDB [RBAC in Business workshop](https://typedb.com/docs/learn-typedb/rbac-in-business/):

- Chapters 1–3 distinguish identities, systems and relationships. Business identity
  remains stable when an authentication method changes.
- Chapters 4–6 model accountable grants and time-dependent assignments as data,
  with schema constraints and audit queries.
- Chapter 7 puts reusable policy logic in TypeQL functions.
- Chapter 8 connects grants to policies or recorded exceptions and models reviews
  as facts about grants.
- Chapter 9 applies those functions in access queries, preserves history by closing
  grants, and uses controlled migrations and idempotent provisioning.

Defining a function alone does not make every arbitrary database query invoke it.
Business uses `dog-typedb::SchemaPolicyAdapter` as its mandatory public data
executor, with verified actor/tenant context and schema-owned read/write policies.
Keep that boundary for frontend, portal and worker data access. Do not replace it
with application role comparisons or a proxy-specific RBAC hook. The workshop's
sample bulk deletions/type replacements are teaching exercises, not instructions
to delete existing Business history.

## Identity and account ownership

A TypeDB person has a stable `identity_id`, name, lifecycle status and optional
contact email. Memberships, teams, grants, policies and reviews remain in TypeDB.
Email contact data is not an authentication credential.

An `external_identity` relationship connects a person or service account to an exact `(issuer,
subject)` pair. The binding key is a length-delimited hash of both values; email
is never used to match two existing authentication accounts. An identity may have
zero or many links. Human and machine actors use the same native data policies. Links are private to trusted account operations, not writable
through arbitrary public queries.

`AuthenticationPlugin` owns proof verification and provider account operations.
`Registry` installs trusted plugins; `dog-auth` dispatches authentication strategies
and binds verified identity to the data request. `LocalAccounts` and `SecretStore`
are separate provider-owned persistence contracts, not business-graph tables.

The managed-provider path requires no local account store. The first integration
is a configurable OIDC JWT access-token verifier suitable for Auth0 or another
compatible provider. Login, signup, password recovery, MFA and provider sessions
remain with the IdP. The frontend must use the provider's supported SDK and
Authorization Code + PKCE flow; this backend does not implement a browser OAuth
callback or accept ID tokens as API credentials.

`GET /authentication` lists configured provider metadata. The custom `account`
method delegates account operations to a plugin. The included OIDC adapter returns
an explicit provider handoff, not a claim that signup/recovery/logout completed.
Provider-specific plugins may implement those operations themselves.

Account linking requires recent MFA on the current account plus a separately
verified proof from the new provider. A link already owned by another person is
rejected; linking never merges two people by email. Business invitations remain
one-use, expiring, revision-checked graph records. A managed-provider invitation
requires a signed, verified matching email. A contact with no authentication links
can receive its first identity link; an already authenticated identity is never
merged by its email alone.

## OIDC configuration

Configure `AUTH_OIDC_PROVIDERS_JSON` as an array. Example (no secrets):

```json
[{
  "id": "jitpomi-auth0",
  "issuer": "https://YOUR-TENANT.auth0.com/",
  "audience": "https://business.jitpomi.com/api",
  "jwks_uri": "https://YOUR-TENANT.auth0.com/.well-known/jwks.json",
  "token_types": ["JWT"],
  "max_token_lifetime_seconds": 900,
  "email_claim": "https://jitpomi.com/email",
  "email_verified_claim": "https://jitpomi.com/email_verified",
  "amr_claim": "https://jitpomi.com/amr",
  "auth_time_claim": "https://jitpomi.com/auth_time",
  "acr_claim": "https://jitpomi.com/acr"
}]
```

Register a distinct API audience and configure short-lived API access tokens at
the provider. The verifier accepts only configured HTTPS issuers/key endpoints,
explicit token types, asymmetric algorithms, signature-verification keys, matching
issuer/audience, valid issue/expiry/not-before times and the configured maximum
lifetime. Keys are bounded and cached for 60 seconds; unknown keys fail closed.
Unverified token claims select only an already trusted provider, never a new URL.

Assurance claim mappings must come from trustworthy provider authentication facts,
not user-editable metadata. Missing MFA/recent-login claims fail a requirement;
the backend does not invent them. Optional `account_url` is a trusted provider
account UI URL, not an automatically constructed OAuth authorization request.

`AUTH_SERVICE_REQUIREMENTS_JSON` controls accepted authentication proofs for a
service, operation or tenant. These are proof requirements, not tenant RBAC:

```json
{"data":{"providers":["jitpomi-auth0"],"operations":{"write":{"methods":["mfa"],"max_age_seconds":300}}}}
```

Plugins verify proof strength; TypeDB still decides access. External-provider
roles and organization claims never become authoritative JITPOMI permissions.
A trusted custom plugin may handle opaque proofs selected with `x-auth-provider`;
that selection does not bypass provider verification or data policies.

Self-contained external access tokens may remain valid until expiry after provider
logout. Use short lifetimes and the provider's session/revocation facilities;
implement a provider-specific online verifier when immediate invalidation is
required. The account handoff does not claim to revoke those tokens. Local
platform-session revocation is separately durable in the optional local plugin.

## Optional local-account plugin

For existing sandbox accounts and migration, `AUTH_LOCAL_ENABLED=true` explicitly
enables the existing password strategies with an external SQLite store. Set
`AUTH_LOCAL_STORE_DIR` to an absolute existing private directory (0700). Files must
be private (0600), and each business database has a separate account namespace.
There is no automatic local fallback when only external providers are configured.
This implementation is for a single host with persistent local storage; do not
share its database over a network filesystem or treat ephemeral container storage
as durable. A different store can implement `LocalAccounts` without changing RBAC.

Password hashes, login identifiers, credential versions, one-use recovery tokens
and session revocations live in that plugin store. Recovery changes the targeted
account; the person's identity and other authentication accounts are unaffected.
Local invitation/recovery ingress remains sandbox-only, as before this refactor.

Financial/notification connection secrets also stay outside TypeDB. The graph
stores `secret_ref`, while an external `SecretStore` holds AES-GCM ciphertext bound
to tenant, connection ID and connection revision. Runtime file storage requires an
existing private `AUTOMATION_SECRET_STORE_DIR` and the original external
`AUTOMATION_MASTER_KEY_B64`. Hosts can supply another secret-store adapter. Back up
the external stores and key separately from TypeDB; losing them loses access to
the credentials. Explicit `Vault::new` uses ephemeral memory for embedded tests;
runtime `Vault::from_env` never silently substitutes memory for persistent secrets.

## Offline migration and cutover

No running development or production database is changed by startup.

1. Stop HTTP writers, automation workers and other database writers.
2. Export schema/data/record evidence and restore-verify the backup. Retain the
   original backup privately, including its encryption key and deployment config.
3. Prepare private persistent account and encrypted-secret directories. Supply an
   operator environment with the exact database, original vault key,
   `BUSINESS_WRITERS_STOPPED=true`, and `BUSINESS_BACKUP_DIR`.
4. Run `cargo run --locked --bin migrate_credentials -- --apply`. The CLI requires
   current backup evidence. It copies account hashes/versions, pending challenges
   and revocations to the external store before removing them from TypeDB, and
   replaces inline encrypted plugin secrets with external references. Conflicting
   external accounts are not overwritten. Both original person-owned credentials
   and the intermediate credential-relationship model are supported.
5. Verify preserved people, memberships, grants, policies, invoices and workflows;
   verify migrated credentials/challenges externally. New identity references
   preserve original person IDs. No business access history is reset.
6. Configure the chosen authentication plugins and deploy the matching binary.
   Verify sign-in, protected data queries, invitation/recovery behavior, account
   linking and worker restart before reopening writes. Switching to the managed
   provider requires sign-in there; old local sessions without an account reference
   are rejected. Verifiable local sessions retain their revocation state.

The migration is repeatable but spans database/store transactions: keep writers
stopped until every phase completes. If interrupted, retain the original backup,
inspect the stage and rerun with fresh stage-specific evidence; never overwrite new
work with an old snapshot. Before reopening writes, rollback can restore the old
schema/data with the matching old binary. Migrating accounts into Auth0 itself and
linking their new provider subjects is a separate, explicit provider import; this
code does not claim that a live Auth0 tenant or password import already exists.

## Verification

Ordinary Cargo/Node checks run without provider/database credentials. Ignored tests
use generated disposable development databases and local TLS identity providers:

```sh
BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test authentication -- --ignored --test-threads=1
BUSINESS_ENV_FILE=/private/dev.env BUSINESS_RUNTIME_ENV_FILE=/private/runtime.env cargo test --locked --test backend -- --ignored --test-threads=1
BUSINESS_ENV_FILE=/private/dev.env cargo test --locked --test billing account_and_invoice_workflow_without_provider_effects -- --ignored --test-threads=1
```

These exercise signed external tokens, invalid proof rejection, identity linking,
TypeDB tenant isolation, account recovery/races, both credential migration models,
external encrypted-secret migration and restart recovery. They are not claims of
live Auth0 setup, sandbox payment settlement or email delivery.
