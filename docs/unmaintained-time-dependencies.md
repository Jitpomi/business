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

## Review separation

The unfinished Business clock migration is excluded from PR #29. Its exact
pre-split state is preserved at `preserve/business-29-before-split`; the clock
changes and upstream research will continue in a separate draft follow-up.
PR #29 retains tenant-owned onboarding, native TypeDB policies, the completed
auth-duration update and payment acceptance safeguards. It does not claim to
complete dependency remediation. The strict dependency-security workflow is
unchanged; a baseline finding is still a failed check, not an exception.
