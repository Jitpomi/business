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
Keep the security gate enabled and leave deployment blocked until all three
remaining findings have a supported resolution. Authentication, native RBAC,
provider uncertainty and independent-tenant billing tests are required for this
partial fix; it must not be described as a completed four-dependency removal.

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

## Separate migration follow-up

This draft is stacked on the tenant-onboarding review (#29). It restores the
previously tested first-party clock migration without mixing it into that
review. The original combined history remains at
`preserve/business-29-before-split`. This is partial progress, not a completed
dependency removal, and must remain draft while the strict audit fails.
