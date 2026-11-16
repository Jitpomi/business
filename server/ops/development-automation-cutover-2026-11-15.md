# Development automation cutover — November 15, 2026

The `jitpomi-business-validation` service is running tested commit
`5f2a8fbd1b7f30f3798700704008a38a8fe1e004` against `business_native_dev`.
Render confirmed the deployment live at 17:28 MST. Production data and services
were not changed. Render's environment label is "Production", but this service's
explicit database target and runtime account are the development ones.

## Preservation and migration

- Suspended the old service and observed HTTP 503 before asserting writers stopped.
- Exported the existing configuration and a fresh schema/data/record backup into
  a private local directory. Restored and verified the backup in a generated
  disposable database before installing the automation schema.
- Installed only the additive automation schema. Imported the existing tenant's
  Stripe TEST and Resend connections, keeping provider aliases and credentials
  bound to the same accounts. Imported billing settings and the billing workflow/
  schedule. The importer verified existing records before and after its writes;
  it made no payment or mail sends.
- Added the approved vault key to the existing Render service. Removed deployment
  connection/settings arrays and the retired worker tenant list. JWT, gateway and
  database credentials were retained, rather than rotated or broadened.
- Exported and restore-verified another backup after deployment. Compared every
  original record against the post-deployment snapshot: all 127 original records,
  attribute names and business values were retained. Only the tenant concurrency
  revision changed. All added records belonged to automation.

Private evidence includes the original/current exports, vault key, record
comparison, API verification and checksums. Credentials, tokens and backup data
are not committed to this repository.

## Live acceptance

Authenticated API checks used a short-lived validation token for the existing
active owner, with the configured token type and current credential version.
No account password, owner membership or existing session was changed.

- Owner automation listing and managed portal resolution succeeded.
- Missing authentication and another tenant's context were rejected.
- Connection credentials/ciphertext were absent from public connection responses;
  generic data access could not expose automation records.
- The retired `/tenants/<tenant>/internal/billing/tick` endpoint returned 404.
- A synthetic workflow completed mapping, filtering, waiting, owner approval and
  a final transform through the live durable worker. Replaying its event returned
  the same run. Its definition was disabled afterward; version/run evidence was kept.
- The imported billing workflow completed with zero errors, zero new invoices and
  zero notices. Existing invoice and notification records remained unchanged.
  This cutover did not create a new Stripe or Mercury transaction or send email.

## Host wake-up

Deployed `jitpomi-business-automation-wake` with a five-minute cron. Its only binding
is the public development service URL, and its only request is `GET /health`.
It contains no billing credentials, payment actions or business scheduler.

Cleared the old `jitpomi-business-billing-scheduler` cron through the Cloudflare API
and read both schedules back: the legacy schedule is empty and the new health-only
schedule is `*/5 * * * *`. The legacy Worker and version history were retained.
