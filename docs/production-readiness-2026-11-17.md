# Production readiness and DogRS structure — 17 November 2026

Status: development validation in progress. This change does not enable live
payments, install live credentials, migrate a database, or certify production.

## Application boundaries

Business follows `dogrs/docs/application-structure.md`:

- `main.rs` owns process startup, logging, the listener, signals and worker drain.
- `app.rs` composes registered DogRS services, their hooks, transports and every
  enabled HTTP ingress. `runtime_router` is shared by the executable and provider
  acceptance suites; `router` remains the reusable service-only constructor.
- `hooks.rs` registers global hooks and exposes transport admission middleware.
  Each service retains its own dog-auth hooks. No application RBAC hook is added.
- `channels.rs` explicitly documents that durable events use the TypeDB outbox
  and dog-queue. There is no ephemeral channel pretending to provide durability.
- `services/` contains authentication, data, financial and automation. Each has
  the common service/hooks/shared/schema modules. Automation command envelopes
  are defined in its schema module; dispatch stays in its service module.
- `plugins/` owns the built-in provider implementations, including Stripe OAuth
  the Stripe/Mercury/Resend automation connectors, and the explicit import of
  older provider settings. Server and import tooling
  share their registration. Generic automation code no longer imports these
  provider implementations; application composition selects trusted packages.

Supporting modules are explicitly allowed by DogRS. They are not extra HTTP
services and should not be deleted merely to shorten the directory listing:

| Module | Responsibility |
| --- | --- |
| `auth/` | Authentication/account plugin contracts, proof requirements and external account lifecycle; credentials remain outside TypeDB. |
| `automation/` | Provider-neutral connection/workflow contracts, encrypted-secret references, native policy adapters, durable workflow state and OAuth handoffs. |
| `background/` | TypeDB queue/recurrence adapters and domain job payloads. Dog-queue owns execution, leases, retry timing and scheduling. |
| `billing/`, `finance/` | Provider-neutral financial contracts and execution of native financial claims. |
| `database/`, migration modules | Explicit provisioning, verification and preservation tools, never implicit startup migrations. |

Workflow definitions, approvals and uncertain external outcomes are domain state;
they are not a second generic job scheduler. Their existing native TypeDB fences,
consent checks, transaction boundaries, stable idempotency keys and effect journals
remain intact. Provider protocols belong in plugins. No public HTTP path, stored
payload, queue job name, schema or customer/provider identifier is renamed here.

The layout guard now rejects flat service modules and missing global modules,
rather than overlooking a service because it lacks a `_service.rs` file.

## Release gates before accepting real customer money

1. **Dependency security:** the separate strict audit must be assessed and resolved
   for production. The remaining Chrono, Chrono-TZ and Chrono-TZ-build maintenance
   findings remain visible. Parser PR https://github.com/typedb/typeql/pull/477 is
   still open as checked on 17 November; a proposal is not an adopted release.
   First-party queue timestamps and legacy parsers remain our responsibility.
   See `unmaintained-time-dependencies.md`. No advisory exception is introduced.
2. **Production deployment:** the current Render service is development validation
   on a free instance. Production availability/capacity, the production database
   and least-privilege runtime account need a separate reviewed deployment.
   The previous decision to retain free hosting is preserved; no paid service is
   purchased by this pass.
3. **Merchant onboarding:** only sandbox Stripe OAuth has been configured and
   accepted. Live platform registration, exact live callback, merchant onboarding,
   credentials and webhook configuration must be verified before activation.
   Tenant authorization and financial consent must continue to be enforced by
   native TypeDB policies, independent of provider account permissions.
4. **Recovery and operations:** demonstrate a complete recovery of the production
   database together with the ciphertext store and the correct encryption key;
   database-only backup is insufficient. Existing backup/rehearsal commands are
   deliberately development-only and must not be passed a production target.
   Configure operator alerts for unavailable services, failed queues, uncertain
   effects and reconciliation delays. Runtime now installs a tracing subscriber
   so framework worker diagnostics actually reach process logs; logs alone do
   not establish an alerting or recovery service.
5. **Money movement:** execute a separately authorized, bounded live acceptance
   after the preceding gates. Sandbox success cannot prove real settlement.
   The retained Mercury bank test is pending; Processing must continue to
   suppress both collection retries and premature receipts. Never overwrite a
   status or submit a second payment to make a check green.

Existing development data and original checkpoint journals remain untouched by
this code refactor. Any later development deployment must use a fresh quiesced
backup and preservation checks, even though this change requires no schema migration.

## Verification of this refactor

- Rust 1.98.0, locked all-target suite: 55 passed, zero failed, 42 separately
  gated tests excluded from that count. Formatting, documentation tests and
  all-target Clippy with warnings denied pass.
- Native development TypeDB: six backend policy/identity/migration/concurrency/
  HTTP checks; local HTTPS OIDC through dog-auth to native multitenant RBAC;
  workflow uncertainty, generic scheduling, OAuth actor/tenant/expiry/replay,
  and failed-verification preservation all pass.
- TLS development cluster: additive backup/install/repeat-install preservation
  and actual provisioning/server/login/data/logout acceptance pass. Runtime uses
  non-admin credentials. No configured database is recreated by these checks.
- Actual website query integration passes through `runtime_router`, including
  invitations, native consent, tenant denial, reconciliation, method-removal
  retry and durable execution. The fixture explicitly selects its generated
  tenant for the existing primary-account URL aliases, just as deployment does.
- Website's 47 tests and Business client/deployed-worker tests pass.
- Fresh generated copies of both retained provider fixtures preserve their
  original graph facts and relationship roles. Real Stripe TEST restart/replay
  passes without duplicate invoices, charges or notices. Mercury sandbox's
  existing Processing payment passes notice/receipt suppression. This is not
  evidence of final Mercury bank settlement. No payment is submitted by this
  pass, and no status is manually overwritten.
- Strict `cargo audit --deny unsound --deny yanked --deny unmaintained` remains
  failing for exactly RUSTSEC-2026-0543, RUSTSEC-2026-0555 and
  RUSTSEC-2026-0559. The audit workflow and its failure policy are unchanged.

Initial local runs exposed missing test inputs: TLS-only backup tools cannot
run against the plaintext loopback fixture, and consent migration requires its
private evidence directory. They were rerun with the documented inputs. One
concurrent build/test run hit local HTTPS fixture deadlines; the unchanged
ordinary suite subsequently passed at its default parallelism. No provider
retry, production TLS relaxation, deadline increase or assertion removal was
used to obtain a passing result.

Private logs, retained source exports, restored provider fixtures and generated
migration evidence are kept outside Git. The deployed development runtime has
not been changed by this code verification, and live-money activation remains
outside this refactor.
