# Attention-run resolution and scheduled backup acceptance — 2026-11-18

PR #35 introduced evidence-bearing cancellation and daily encrypted checkpoints;
PR #36 added bounded, credential-redacted failure diagnostics. Development is
live at backend commit `995303ad48cc37e164df8098270d9bff58bea608`, deployment
`dep-dbuotgafngtc73deq5fg`. Later main changes affect backup diagnostics,
monitor configuration, tests and documentation. Real payments remain disabled.

## Six historical attempts

The six attention runs were single-claim, at-most-once attempts at the same
aggregate billing cycle on 2026-11-16. Three retained an uncertain-outcome error;
three retained a connector-action failure. Their external completion was not
proved and was not fabricated. Read-only observations through the tenant's own
Stripe plugin confirmed that all four retained invoices matched paid provider
IDs and amounts and predated these attempts.

After a full restore rehearsal, development writers were paused and a fresh
checkpoint was taken. A single native control function was updated in a schema
transaction; its graph comparison was identical before and after. Each run was
then cancelled through the native owner-scoped control path with recorded
evidence. No provider action, invoice/payment-status override or notification
send was used to close them. Their original attempt count, idempotency key,
workflow revision, cursor, owner and error are retained. Later worker delivery
is a no-op for each cancelled attempt.

The source comparison retained all 1,771 entities/relations. Exactly the six
run payloads/status/revisions and the existing tenant concurrency revision
changed. Every invoice, payment, notification, identity, role and other record
was identical. The concurrency revision remains part of the existing native
write-conflict protection; it was not disabled for maintenance.

## Daily offsite backups

The enabled `Business encrypted backups` workflow runs daily at **09:17 UTC**
and retains recipient-encrypted artifacts for **30 days**. It runs independently
of an operator computer. A failure sends an operations email; a checkpoint over
36 hours old is visible to both the Cloudflare monitor and the independent
watchdog. Success is acknowledged only after graph rehearsal, encryption and
artifact upload. An old receipt cannot advance freshness.

The job receives a non-admin TLS database connection and dedicated snapshot
credentials. Runtime configuration is a pre-encrypted capsule. The recovery
private key and plaintext vault master key/payment credentials are not provided
to GitHub or the snapshot workers. The new recovery identity is a separate
private file for the password manager; old manual recovery keys remain useful
for their older archives. Re-seal configuration after credential changes.

The first remote attempt, run 44296573934, failed without publishing an archive
or success receipt. Its failure email was verified in the operations inbox.
The initial wrapper hid the command detail, so an exact cause is not claimed.
Bounded redacted diagnostics now expose subsequent command failures. Both the
following instrumented run 44297221429 and the final **main** run
[44297924916](https://github.com/Jitpomi/business/actions/runs/44297924916) passed
export, isolated graph rehearsal, encryption, upload and acknowledgement.
No verification requirement, TLS check or native policy was bypassed.

The downloaded main artifact's SHA-256 matched its acknowledged receipt.
Its complete graph matched the post-resolution development checkpoint, and all
11 manifested files verified. A second owner-only encrypted Drive copy was
uploaded, downloaded again and hash-matched; the recovery key was not uploaded.
Drive is a verified second copy of this checkpoint, not a claim that every
future GitHub artifact is automatically replicated there.

## Recovery and safety tests

An independently downloaded encrypted offsite archive was decrypted with the
separately held identity, including its sealed configuration capsule. A new
isolated TypeDB database restored the complete graph and native schema, with
all six cancellation audits intact. The independent ciphertext copy retained
20 rows; all 16 connection credentials and the operations canary decrypted,
while wrong-key/wrong-tenant checks rejected access. The recovered app used
that copied store, without contacting the original secret service.

Fresh Auth0 machine login, native own-resource reads, foreign-resource
filtering, foreign-tenant denial and native membership revocation passed on
the recovered app. The token was still unexpired when revocation rejected it.
Recovery mode withheld workers and rejected payment writes and webhooks. Only
the isolated recovery listener was stopped; audit checkpoints were retained.

All-target Rust tests, warnings-denied Clippy, the real native uncertain-action
control test, the real online checkpoint/policy-update/recovery test, 16
monitor/store/watchdog tests and five Python recovery tests passed. Native
fixtures were corrected to respect ownership constraints; no constraint or
assertion was removed to get a pass. Missed-backup incidents, recovery receipts
and the independent stale-backup warning are tested without real notifications.
The actual remote failure path also delivered its operations email.

Final operational status: dependencies ready; zero attention runs, failed jobs,
failed runs, overdue jobs, attention invoices or stale reconciliations; fresh
acknowledged backup; zero pending alert deliveries. The attention-run recovery
email was verified in dev@jitpomi.com's inbox. All 14 financial connections
remain sandbox connections, and the Stripe platform credential remains a test
key. The three existing upstream maintenance advisories remain visible in the
strict dependency audit. This work does not authorize live-money activation.
