# Historical verification and review investigation

The following records describe the October 2026 runs and dependency changes.
Their test counts, pins and provider availability describe those runs, rather
than the current checkout or deployment. Use `../README.md` and `../Cargo.toml`
for current setup and dependencies.

### Verification evidence — October 20, 2026

The 0.3.4 follow-up reproduces absent-policy deletion as HTTP 500 on the old
dependency and verifies HTTP 403, client redaction and unchanged persisted state
on the published fix. All four real backend tests, default tests, Clippy and
frontend tests pass with the published dependency and no local override. Real
Stripe test-mode HTTP acceptance covers authenticated customer CRUD/idempotency,
unconfirmed intent cancellation, Checkout expiration and draft invoice/item
reads with synthetic cleanup. No payment is confirmed or email sent. Mercury's
direct sandbox baseline still returns HTTP 401 with an IP restriction; do not
describe that as passing provider acceptance. See `tests/README.md` to rerun the
explicit provider checks.

All-target debug builds, release builds of both executables, formatting and
Clippy with warnings denied pass. The final published-0.3.3 dependency run passed
29 library tests, two layout tests, all four real TypeDB backend tests,
and three frontend-client tests. The real Stripe test-mode lifecycle passed in
the earlier consolidation run; it was not repeated for the compiler-only patch.
All Rust source/test files are reachable from Cargo targets; disconnected
modules and old test files are no longer silently outside build coverage.
Generated test databases were cleaned up. No production database, live payment,
deployment or external credential configuration was changed.

### Review failure investigation and fix

The first review 500 lost its underlying cause, so later passing runs were not
accepted as a resolution. Focused concurrent reviews subsequently reproduced a
30-second pre-commit deadline failure without fault injection. A controlled
probe then added 250 ms to each compiler schema-introspection round trip: the
old compiler made 42 such requests before its first mutation, and all four
concurrent reviews returned the published executor's 500 at about 31 seconds.
Compilation, not execution of the mutation, consumed most of the budget.

dog-typedb 0.3.3 fixes that excessive introspection by reusing immutable schema
metadata inside the same transaction and across its checkpoints. It never caches
frontend queries, policy decisions, authentication, membership or application
data, and never reuses metadata across transactions. The identical latency probe
needed 9 lookups for the first checkpoint and 12 for the entire review write;
attempts completed in roughly 6–8 seconds without increasing the 30-second limit.
The probe deliberately kept the old timeout-to-500 mapping, demonstrating that
the result is not merely a changed HTTP error classification. No write was retried.

The fix also retains private error causes and execution phases, keeps them out
of client responses, and returns 409 only for TypeDB's structured STC2 commit
rejection. Lost connections and commit deadlines remain unknown outcomes; the
framework never automatically replays them. The tenant revision update remains
in place, preserving conflict detection between concurrent policy changes.

Regression coverage includes real compiler round-trip reuse and fresh transaction
metadata, nine distinct reviews, successful-repeat deduplication, denied self-review
with unchanged history/revision, concurrent authenticated service calls, real HTTP
reviews, and reconciliation of every response against persisted history. Before
publication, all five framework live tests and all four Business backend acceptance
tests passed (Business: 187.46 seconds). The original historical request cannot be
retroactively attributed with certainty because its cause was discarded; the
reproduced review timeout now has a demonstrated code fix and before/after evidence.

dog-typedb 0.3.3 is published on crates.io and pinned exactly in Business's
manifest and registry lockfile. A fresh run against the downloaded release,
without a local patch, passed all four backend tests in 186.74 seconds. Debug
and release builds, unit/layout/frontend tests, formatting and all-target Clippy
also pass. No dependency other than dog-typedb changed during this patch upgrade.
No application deployment or live payment was performed.

The real Mercury sandbox check remains blocked: both the service chain and a
direct accounts request were rejected by the provider's IP allowlist (direct
HTTP 401). Local Mercury auth, resource and HTTPS contracts pass. Do not call
this external acceptance check green or weaken the allowlist to bypass it.

