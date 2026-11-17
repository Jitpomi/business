# Business policy audit

This change targets the remaining billing and workflow decisions after `/portal`
retirement and the generic dog-queue scheduling migration.

| Responsibility | Authority after this change |
| --- | --- |
| Tenant membership, actor scope, financial buyer/reader access | Native TypeDB functions; repeated in transaction predicates |
| Workflow ownership and publication | Native current-owner predicate at insertion/write |
| Approval, resume, cancel, evidence and cursor transitions | Native run control/progress functions |
| Retry eligibility and uncertain external outcomes | Native recovery/failure decisions; persisted claim and journal |
| Connection capacity, schedule target and interval, reminder interval | Native insertion/update predicates |
| Disabled event origins | Native origin status, enabled flag and immutable definition checked when inserting runs |
| Invoice issuance and collection eligibility | Native policy repeated in atomic claim |
| Consent revision and plugin/method binding | Native billing functions and claim predicates |
| Provider observations | Native amount/currency/binding and terminal-state transition predicates; plugin protocol validation in Rust |
| Invoice, reminder and receipt eligibility | Native notice decision repeated at claim, including imported-history suppression |
| Next billing sequence and business interval | Native TypeDB function |
| Calendar date arithmetic, recurrence, delay, execution, leases and supervision | Generic dog-queue SDK |
| JSON decoding, bounded requests, wire identifiers, URL/SSRF checks, secret encryption, provider I/O and message copy | Rust and tenant-owned plugins |

The workflow payload remains the execution document. Typed policy facts are
written with it in the same transaction; definition and approval-history hashes
prevent ordinary CAS paths from replacing immutable execution input or skipping
approval. Public `/data` cannot edit these hidden automation records. Background
outcome journaling remains possible after owner revocation; claiming new external
work requires current native authorization.

## Preservation and verification

- Restored the immutable development backup in an isolated TypeDB 3.14.1 server:
  1,644 original objects and all original graph facts preserved; 1,419 records
  received additive policy projections. A second backfill changed zero records.
- The migration extends the exact exported schema. It preserves custom functions
  and rejects conflicting policy definitions rather than replacing them.
- Native adversarial tests bypass service preflights: revoked writers, skipped
  approvals, terminal-state reversal, mismatched payment observations, imported
  history notices, uncertain-write retries and insufficient evidence are denied.
- Two independent tenant-owned financial/notification plugins and the website
  account flow pass native TypeDB and HTTP tests, including cross-tenant denials,
  consent, collection, receipts, reminders and method removal.
- Workflow execution and generic schedule dispatch pass with real TypeDB and
  dog-queue. The explicit backup/installer binary test also passes against hosted
  development TypeDB with certificate validation unchanged.
- A loopback-only test helper permits an explicitly selected local fixture. It
  rejects remote addresses under that flag and preserves the supplied database
  credentials. Production connection configuration is unchanged.

These are native/local plugin acceptance results, not a new live Stripe/Mercury
payment proof or a live database cutover. The running development service and
source database remain unchanged. Apply the documented v3-to-v4 shadow migration
only after quiescing writers and verifying a fresh backup; startup will reject
incomplete native policy facts rather than silently backfilling them.
