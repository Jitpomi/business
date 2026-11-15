# Backend maintenance audit

Reviewed application composition, authentication/transport parameters, data and
portal services, provider registration, billing/storage, schema references,
operator tools, deployment configuration and test/CI wiring. This is a source
and local-test audit; deployed state was not inspected.

## Cleanup

Removed no-op startup modules, duplicate startup validation and unread builder
settings in the first pass. Moved the test-only Tower dependency to development
dependencies. This pass removes unread inner request headers/method/path/raw-query
fields and the unused Render ENVIRONMENT setting. Authentication retains outer
AuthParams headers, provider filters retain inner query, and credential guards
retain both provider markers. Historical verification notes are preserved in
verification-2026-10-20.md. CI now includes existing scheduler tests and runs when
the scheduler changes.

## Useful work retained

- The additive migration preserves identities and invoice history, pauses imported
  plans and keeps historical invoices from becoming new billing work. Its removal
  requires confirming migration and recovery needs across existing targets.
- The account-link installer updates an existing canonical sandbox without a
  destructive rebuild. New databases include the schema during provisioning.
- The external scheduler wakes sleeping hosts; the in-process loop cannot replace
  that function. Admission and database claims protect overlapping invocations.
- User-facing provider proxies and merchant-owned billing execution have different
  authorization and lifecycle responsibilities.
- Consent revisions, payment-method tombstones, durable claims and unknown-outcome
  handling protect against repeated charges and stale authorization.
- TypeQL assets are frontend examples, not a query authorization registry. Schema
  functions can be useful to operators without a direct Rust reference.
- External integration tests are explicitly gated rather than dormant tests.

No Rust source or test file was found outside the active Cargo target/module
graph. No public function name appeared only at its declaration in the source/test
reference scan. These checks do not prove that every exported API or database
function is used externally; uncertain candidates were retained.

## Validation

40 Rust tests and five JavaScript tests passed. Seven external integration tests
were intentionally ignored. Formatting, all-target build, Clippy with warnings
denied and documentation tests passed. No migration or live payment was run.
