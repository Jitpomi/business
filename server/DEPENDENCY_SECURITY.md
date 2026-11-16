# Dependency security validation

Verified 16 November 2026. The active application graph has zero reported
vulnerability, unsoundness, yanked or unmaintained findings. CI rejects all four
classes without advisory ignores or target filtering.

Business pins JITPOMI's reviewed TypeDB driver compatibility commit `548a885`
through its root Cargo patch. It selects matching protocol commit `2644e0d`.
These branches retain the exact upstream 3.13.6 Rust source and unchanged 3.13.4
protobuf definitions, migrating their Tonic/Prost transport to maintained 0.14.
Certificate roots, hostname verification and authentication injection remain;
there is no custom TLS/cryptography implementation. The native schema, RBAC,
credential formats, plugin ownership and payment idempotency keys are unchanged.
No data migration or live credential-store rewrite is required.

The source branches are explicitly JITPOMI-maintained, not upstream releases.
Their JITPOMI-MAINTENANCE.md documents provenance, validation and replacement.
The driver removes the unused normal async-std dependency, matching the prior
registry package, and preserves the C client's sync feature at its own boundary.
The pin must be replaced only by an equivalent maintained upstream release that
passes the same TLS, native policy, Auth0, migration and provider acceptance tests.
Cargo root patches are workspace scoped; this does not claim to change already
published DogRS crates or constitute a new crates.io release.

Verify with `cargo audit --deny unsound --deny yanked --deny unmaintained`, Rust
and frontend checks, then the explicit native-policy, credential preservation,
consent, two-tenant plugin and provider replay suites described in tests/README.md.
Replay retained Stripe/Mercury IDs; do not replace invoices, force settlement or
repeat notices. Existing data stores and migration checkpoints remain retained.
