# Development cutover — 17 November 2026

The development service is live on commit `4ea92664997d1dd27254e04258219dc46fcc2332`
([Render deployment](https://dashboard.render.com/web/srv-db3tstrbc2fs73buj2dg/deploys/dep-dbu7vl8m7kps73b8rfv0)),
using `business_native_dev_v5`. The deployment manifest matches that database.
Automatic deployment remains off; the free instance and existing runtime
credentials, master key and durable encrypted secret store are retained.

## Preservation

Writers were suspended before exporting a fresh v4 backup. The backup restored
successfully. Both rehearsal and cutover verified all 1,694 original objects and
relationship roles; a second backfill made no changes. Migration made no provider
calls. Final read-only checks confirm v4 is unchanged and its original graph is
preserved in v5. All 69 completed queue jobs and four original encrypted
connections are retained. Verification shadows and original provider checkpoints
remain available; nothing useful was deleted or overwritten.

## Verification

- Fresh independent human Auth0 password/PKCE logins passed native TypeDB RBAC
  acceptance. The deployed service passed authentication, own-resource access,
  foreign-resource filtering, foreign-tenant denial, protected-write denial,
  duplicate-resource 409, retired-route 404 and connection-catalog checks.
- Final deployed checks also confirm revoked synthetic membership returns 401,
  malformed/unsupported OAuth returns clear errors, and health returns 200.
- Real Stripe restart/replay passes without extra charges, invoices or mail.
  Mercury Processing suppresses premature receipts; the separate existing
  provider-Paid fixture retains exactly one receipt and its original delivery ID.
  These provider workflows use isolated cloud TypeDB checkpoint copies.
- Direct connection verification from the running Render service passes for
  both Stripe and Mercury. A deliberately invalid sandbox credential returns
  an actionable 409 and never marks its connection verified.
- The verification fix passed 54 ordinary Rust tests, all-target Clippy,
  correctness CI and new native failure/isolation tests against local and
  cloud TypeDB. External-service tests remain explicitly gated.

## Hosted Mercury issue resolved

Direct verification exposed a generic HTTP 500. PR #31 classifies provider
verification failures without exposing provider bodies, URLs or credentials:
provider rejection is 409; unavailable/rate-limited verification is 503; other
provider failures are sanitized 502. Failed verification preserves stored
connection state. Tenant and owner checks remain enforced by TypeDB.

Mercury rejected Render with HTTP 401 because the sandbox token allowed only
three workstation IPs. The explicitly approved sandbox-only additions are
`74.220.48.0/24` and `74.220.56.0/24`, the shared outbound ranges shown by Render.
All three existing entries were retained. After that change, the same deployed
Mercury verification returned 200. No production token was changed.

Synthetic test memberships are closed, original identity and Auth0 blocked
states restored, and final test plugin connections disabled. Audit fixtures
remain retained. The loopback login-proof collector has been stopped.

## Remaining limits and rollback

The bank transfer is still pending; no payment was resubmitted and no provider
status was overridden. The separate already-Paid sandbox case is not proof of
bank settlement. A real third-party OAuth authorization-code exchange was not
performed by this cutover; the shared OAuth policy tests and hosted error paths
were verified.

The strict audit still reports the same three accepted maintenance findings:
Chrono, chrono-tz and chrono-tz-build. No advisory suppression or RBAC bypass was
introduced. See [dependency ownership and upstream work](unmaintained-time-dependencies.md).

Retain v4, its backup and verified shadows. Before rollback, quiesce writers and
reconcile any work added to v5; never blindly switch the service back over newer
work. This record changes documentation only; the deployed runtime remains the
verified commit above.
