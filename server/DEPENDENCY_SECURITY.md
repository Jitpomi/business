# Dependency security validation

The November 2026 update refreshes the registry lockfile and upgrades bundled
SQLite through rusqlite 0.43. It removes the known vulnerability and unsoundness
findings without changing the authentication schema, credential format, native
TypeDB RBAC, tenant plugin ownership or payment idempotency keys. Existing auth
stores and database checkpoints remain outside source control and are retained.

CI audits the lockfile and rejects vulnerabilities, unsoundness findings and
yanked dependencies. No advisory is ignored. RustSec still reports the
unmaintained `rustls-pemfile` 2.2.0 dependency through TypeDB's driver and tonic
0.12. The current upstream TypeDB driver also uses this tonic series. There is
no patched pemfile release; removing the warning requires an upstream driver/
transport migration. We retain verified TLS and the compatible database protocol
and do not substitute a local crypto/transport fork or disable TLS to hide it.

Verify with `cargo audit --deny unsound --deny yanked`, the ordinary Rust and
frontend checks, then the explicit synthetic native-policy and provider replay
suites. Replay existing Stripe/Mercury checkpoint IDs; do not create replacement
invoices or force provider payment status. The canonical consent and migration
acceptance guides describe those operator workflows.
