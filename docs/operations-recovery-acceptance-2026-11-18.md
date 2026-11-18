# Operations and recovery acceptance — 2026-11-18

PR #34 adds read-only readiness, independent operational monitoring and a
separate watchdog, plus encrypted full-graph recovery tools. The native schema,
tenant authorization and provider payment rules are unchanged. Real payments
remain disabled.

Verification completed before deployment:

- All-target Rust tests, warnings-denied Clippy, 10 monitor/watchdog tests and
  manifested archive tests passed. The eight hosted native backend checks passed
  (the provisioning check was rerun with its required runtime credential file).
  The local Docker fixture had a read-only filesystem after a disk-full event;
  the native recovery test was rerun successfully on the development TLS cluster,
  never by relaxing TLS or native policy checks.
- Quiesced development checkpoint: all 1,771 existing entities/relations and
  attributes retained. Complete graph import and native runtime verification
  passed in new generated databases. The source preservation check passed after
  recovery, without changing source payment statuses or journals.
- An authenticated encrypted archive was uploaded to the owner-only recovery
  Drive folder, downloaded independently and hash/manifest verified. The full
  downloaded graph restored. The entire 20-row ciphertext SQL export matched
  the new remote D1 store in every column; all 16 connection credentials decrypted
  with wrong-key and wrong-tenant rejection.
- The actual recovered app started using the shadow database and independent
  vault. Fresh Auth0 machine authentication, own-resource access, native foreign
  resource filtering, foreign-tenant rejection and membership revocation passed.
  Recovery mode withheld background workers and rejected writes/webhooks. Wrong
  vault keys returned a redacted 503.
- A durable monitor drill produced exactly one outage and one recovery message
  despite repeated requests. Both provider acknowledgements and both messages
  were verified in dev@jitpomi.com's inbox. Restoring the monitor SQL retained
  delivery acknowledgements and original idempotency IDs.
- Real Stripe TEST restart/replay passed without extra invoice, charge or mail.
  Mercury's actual sandbox payment remained Processing; repeated checks correctly
  withheld reminders and receipts. This does not prove final bank settlement.
- Correctness passed on merged main in run 44255801805. The separate strict
  dependency audit remains visibly failing for the same three upstream
  maintenance advisories, with no exclusions or security-policy changes.

The recovery key is a separate private file intended for the password manager;
its import must be completed by the owner. The encrypted checkpoint is not an
Auth0/password-database export or provider account backup. A lost website session
key requires rotation and fresh login. See operations-and-recovery.md for exact
restore, verification, uncertainty and external-dependency procedures.

Development deployment `dep-dbukrf49v7es738ssl60` is live at commit
`1fa92f2cdd7b862daa8b9fb871ba56b352b4155e`. Protected readiness is healthy;
anonymous readiness is denied. The five-minute Cloudflare cron and GitHub
watchdog are enabled. Independent watchdog drill run 44256558603 passed, and
its separate labelled email was verified in the operations inbox. The monitor
also delivered an actual alert for six retained attention runs. Other observed
metrics (failed/overdue jobs, failed runs, attention invoices and stale
reconciliation) were zero. Pending alert delivery was zero. Those six existing
runs were neither erased nor retried to manufacture healthy metrics.

The final owner-only encrypted offsite package contains 29 manifested files,
including exact backend/frontend source archives and monitor journal/config.
Its independent download hash matched, all files authenticated, and another
new generated database restored the complete graph and native runtime schema.
All 16 connection credentials and the vault canary verified from the downloaded
20-row ciphertext export. Database import plus vault verification took 25.95
seconds in this rehearsal; this excludes infrastructure setup, provider
control-plane access and customer acceptance, so is not a full RTO commitment.
The fresh source preservation check after deployment still retained all 1,771
records. Temporary recovery HTTP listeners were stopped; source and audit
checkpoints were retained.

The checkpoint is a deliberate quiesced backup, not a promised continuous or
scheduled backup/RPO service. Define the production backup cadence, retention
and independent provider-account recovery before accepting real money. Existing
upstream maintenance findings, retained attention runs and Mercury settlement
limits remain visible. This deployment does not authorize live payments.
