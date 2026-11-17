# Time dependency audit — 17 November 2026

The dog-auth update replaces humantime-serde/humantime with registry fundu 2.1.1
and replaces dog-auth's chrono clock calls with checked std::time Unix seconds.
The 0.4 framework and compatible 0.3 backport have the same change. Business
pins the immutable 0.3 backport without changing framework API versions. The
matching dog-core source is byte-for-byte identical to registry 0.3.0; its pin
keeps authentication and local-auth on one core type identity. No third-party
fork or advisory suppression is added. A registry patch release would require
separate publication approval.

Existing human duration settings retain their unit aliases, Unicode whitespace,
fixed month/year meanings and nanosecond precision. Serialization remains a
string, normalized to exact seconds. Ambiguous, signed, infinite, exponent and
out-of-range values fail closed. JWT positive lifetime, checked expiry addition,
issuer/audience checks, revocation and single-use refresh remain tested.

## Remaining upstream barrier

The strict audit is still failing for chrono, chrono-tz and chrono-tz-build.
Business itself and dog-queue also use chrono. Changing only those direct uses
would not eliminate the TypeDB dependency paths:

- typedb-driver → chrono-tz → chrono-tz-build
- typedb-driver → chrono
- typeql → chrono
- dog-typedb → typedb-driver/typeql

Registry typedb-driver 3.15.0 and typeql 3.15.0 were inspected, alongside the
current official upstream manifests. They still require the same time crates:

- https://github.com/typedb/typedb-driver/blob/c7107753213f1d89e0f837bbe02e45df2988a06f/rust/Cargo.toml
- https://github.com/typedb/typeql/blob/7de03ede9d849a793603d9d4406a6268a01ddd67/rust/Cargo.toml

There is no verified upstream-only drop-in migration that clears these findings.
The remaining work needs an upstream-supported driver/parser time migration,
followed by migration of DogRS/Business time types and compatibility testing.
An HTTP transport alone would not remove TypeQL's parser dependency. The parser
is part of the authorization compiler; removing validation, using string
rewrites, relabelling packages or filtering the audit would undermine the
requested constraints. No such workaround was made. Existing TypeDB transport
pins remain unchanged; this change does not claim to retire them.

No runtime database or checkpoint is migrated, reset or deleted by this change.
Keep the security gate enabled. Samson accepted the existing maintenance
findings for merging verified first-party fixes while upstream work continues;
this does not turn the failing audit into a pass or authorize deployment.
Authentication, native RBAC, provider uncertainty and independent-tenant billing
tests remain required; this is not a completed four-dependency removal.

## Upstream submissions and first-party clock migration

The active upstream submission is a bounded parser cleanup:
https://github.com/typedb/typeql/pull/477. It removes an uncalled crate-private
helper and its direct Chrono dependency, preserving the existing module path.
It adds no replacement library or build-system change.

The driver discussion at https://github.com/typedb/typedb-driver/issues/950 now
asks maintainers about supported contracts, migration options and release scope.
The broader prototypes have been withdrawn as merge proposals; their code and
selected-case test material remain available as reference:

- https://github.com/typedb/typedb-dependencies/pull/640
- https://github.com/typedb/typedb-protocol/pull/265
- https://github.com/typedb/typedb-driver/pull/951

The archived build prototypes used contribution-fork pins only for review. The
active parser cleanup has no such pins. Business has not adopted those forks.
The current released driver/parser still contain the flagged dependencies; a
supported upstream migration or another verified integration is needed before
this graph can be called clean. The prototype's selected passing tests do not
establish a complete SDK or cross-language compatibility contract.

Business's trusted Unix-second clock and native TypeQL UTC inputs now use the
maintained time library. Native inputs retain UTC and truncate to the existing
six fractional digits. Authorization and tenancy decisions remain in TypeDB.
The legacy date parsers and dog-queue public/persisted timestamp interfaces still
use Chrono; their migration is not represented as finished by this clock change.
No package alias, advisory exception, removed parser or success-only audit filter
is used to hide these remaining paths.

Validation of the clock change: 53 ordinary tests, Clippy with warnings denied,
live hosted authentication-to-RBAC, native OAuth tenant/actor/expiry/replay checks,
and two tenants billing through independent plugins pass. Real retained-provider
replay initially stops at the canonical-schema check because its checkpoints
predate onboarding. A separate shadow rehearsal, using a verified Stripe backup
and fresh generated targets only, encountered hosted TypeDB gateway 502/404
errors. Original checkpoint roots, invoice/payment IDs and source databases are
retained. No provider status override, payment submission, live-schema cutover or
runtime fallback was performed. These failed hosted checks are not called passes.

The strict dependency-security check remains a required failing gate until the
remaining first-party timestamp migrations and official upstream adoption are
complete. Do not merge or deploy this as a completed dependency removal.

## Merge scope and remaining ownership

PR #29 merged tenant onboarding, the completed authentication-duration fix and
payment safeguards. PR #30 brings the verified Business clock changes into main:
trusted Unix seconds and native UTC literals use time 0.3.55, retaining UTC and
six-digit precision. No TypeDB temporal prototype is adopted. Original commits
and the full pre-split state at `preserve/business-29-before-split` are retained.

### Awaiting TypeDB

- Parser cleanup and maintainer review: https://github.com/typedb/typeql/pull/477.
- Driver temporal compatibility and supported migration direction:
  https://github.com/typedb/typedb-driver/issues/950.
- Supported releases must remove the driver's Chrono/timezone paths and the
  parser's Chrono path before normal dependency adoption can clear that graph.
  Acceptance of a proposal is not the same as an available compatible release.

### Still owned by JITPOMI

- Legacy date parsing/formatting in Business remains on Chrono.
- DogRS queue public/persisted timestamps and their Business adapters still use
  Chrono; this is a first-party compatibility migration, not a TypeDB task.
- Adopt compatible supported upstream releases when available, verify timestamp
  and parser behavior, rerun the strict audit and payment/RBAC regressions.
- Framework publication, development migration and deployment remain separate
  actions. This merge performs none of them.

The existing first-party fixes can ship to main independently of that future
work. It would be inaccurate to say all remaining work belongs to TypeDB.
The strict audit remains unchanged and reports chrono, chrono-tz and
chrono-tz-build. Native TypeDB RBAC and payment uncertainty safeguards remain
in force; a pending bank payment never counts as settled.
