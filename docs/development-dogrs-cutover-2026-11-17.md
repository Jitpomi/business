# DogRS refactor development deployment — 17 November 2026

PR [#33](https://github.com/Jitpomi/business/pull/33) is merged. Development runs
`a2d20eca07a220a68390368aeeee2b478e50621b` on the existing
`jitpomi-business-validation` service. Render deployment
`dep-dbujka0m7kps73cjuulg` succeeded, and its UI identifies that exact source
commit. The customer frontend remains the previously deployed account frontend;
no frontend deployment or new framework release was needed.

## Safe deployment

The development service was suspended before exporting a fresh snapshot of
`business_native_dev_v5`. Its backup was restored into a generated verification
database, verified, and that disposable verification database removed. The
original backup is retained privately. No schema migration, database replacement,
startup reset, encryption-key rotation or environment-variable change was needed.
The existing free hosting plan and manual deployment setting remain in place.

Before deployment and again after acceptance cleanup, all 1,753 original records
retain their attributes and original graph facts/relationship roles. This includes
69 original queue jobs. All 12 prior encrypted credential references remain
readable. Acceptance retained four additional encrypted test connections as audit
history; all four are disconnected. The final vault check decrypts all 16 retained
references without exposing their contents.

All financial connection configurations and the three existing billing settings
remain sandbox-only. The platform Stripe credential is a test key. Local password
authentication stays disabled, database TLS remains enabled and startup recreation
remains disabled. No real payment was enabled or submitted, and no provider status
was overridden.

## Acceptance after merge and deployment

- Merged-main [correctness CI](https://github.com/Jitpomi/business/actions/runs/44245311193)
  passes. The separate [strict dependency audit](https://github.com/Jitpomi/business/actions/runs/44245311203)
  still fails for exactly RUSTSEC-2026-0543, RUSTSEC-2026-0555 and
  RUSTSEC-2026-0559. No finding is suppressed or folded into correctness.
- Deployed health returns 200; anonymous data access returns 401; the retired
  `/portal` route returns 404.
- Short-lived machine authentication and a fresh hosted human PKCE login both
  pass through deployed dog-auth to native TypeDB policies. Own-resource reads
  succeed, foreign resources are filtered, foreign-tenant access is denied,
  protected identity writes return 403, and duplicate insertion returns a
  structured 409 without changing the existing resource.
- Deployed connection discovery and structured OAuth error handling pass. Fresh
  isolated tenant Stripe and Mercury connections verify through the actual
  provider read probes from Render. The independent notification plugin passes
  its configuration verification. Foreign-tenant connection access is denied;
  credentials are absent from public connection inspection. No verification
  email, invoice or payment is sent by these probes.
- The deployed account frontend completes hosted sign-in, loads the synthetic
  workspace and displays **Test mode — No real payments**. The connection catalog
  groups the providers and exposes the configured Stripe OAuth entry. Existing
  native OAuth fencing remains unchanged; this pass does not create another
  Stripe account or accept new connected-account terms.
- Closing the synthetic customer's native membership while its browser session
  is still fresh makes Refresh return to sign-in. A separate request with its
  unexpired Auth0 token returns 401. Both test memberships are closed and both
  original identity statuses restored. The synthetic Auth0 account is blocked
  again; its original email-verification flag is unchanged. The local proof
  collector is stopped.
- Stripe TEST restart/replay and Mercury sandbox Processing checks pass again
  using the merged binary and fresh generated copies of the retained provider
  fixtures. Source snapshots, graph facts and relationship roles are preserved.
  Stripe produces no duplicate invoice, charge or notice. Mercury produces no
  premature receipt or repeated reminder. These restored-fixture tests exercise
  the same application/router and tenant plugins; they are distinct from the
  deployed endpoint checks above. Actual Mercury bank settlement remains
  unverified and is not represented as Paid.

Chrome disconnected during validation. Render's successful deployment was verified
in the signed-in development Chrome profile before that disconnect. The fresh
customer sign-in/workspace/revocation checks subsequently completed in an isolated
browser session using the same synthetic account and deployed public frontend.
No authentication protection was relaxed to work around the browser connection.

## Rollback and production limits

The prior working runtime deployment `dep-dbucvhec6qcc73dcmsbg` remains available.
Since this refactor changes no schema or persisted job contract, rollback is a
binary rollback after draining writers; do not restore an older database over
new work or discard uncertain-effect journals. Preserve the ciphertext store and
its unchanged encryption key together with the database backup.

This is a verified development deployment, not live-payment approval. The
production gates in [the readiness review](production-readiness-2026-11-17.md)
remain open.
