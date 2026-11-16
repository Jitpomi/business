# Portal retirement — implementation checkpoint

This is in progress. Do not remove `/portal` or deploy the website branch on its
own. Development and production data have not been migrated by this checkpoint.

Implemented:

- Account discovery is a frontend-owned TypeQL query through `/data`. Native
  context functions return the current person's own profile and active company
  labels. The existing strict tenant scope is unchanged.
- `/financial` owns runtime mode, payment-method listing/setup and invoice
  history. Native billing functions authorize access before provider calls.
  Shared billing access contains no service-layer role list.
- The website uses those boundaries. Card-specific setup/confirmation aliases
  are removed in favor of generic payment-method names.
- Existing invitation, consent, removal, provider binding and reconciliation
  behavior is retained while its replacement is built.

Remaining before retirement:

1. Add native creation/renewal/revocation policies for billing customers, buyer
   tenants and invitations. Keep proof-checked acceptance in the authentication
   plugin boundary; keep credential-bearing links outside TypeDB and workflow
   results. Preserve existing invitation IDs, revisions and used/expiry records.
2. Persist provider-verified method facts, then enforce default selection,
   automatic-payment consent and removal intent through native TypeDB policies.
   Preserve stale-revision rejection, customer/provider binding, consent evidence
   and retry-safe removal. Frontends must not manufacture verified method facts.
3. Move confirmation's provider work to `/financial` and replace its implicit
   default-setting write with an explicit native operation. Keep save-only setup
   separate from authorization to charge.
4. Use the existing automation engine for reconciliation; retain its observation
   validation and uncertain-effect handling. Do not introduce another job engine.
5. Replace the website's older local password flow with hosted Auth0 login and
   managed invitation acceptance. Current local-auth mock tests do not establish
   compatibility with the Auth0-only development service.
6. Audit the website's global owner/customer view: a tenant can both sell and buy.
   Preserve access to customer payment preferences when adding merchant features.
7. Rehearse an additive migration on a restored backup. Before live development
   changes, stop writers, take and restore-verify a fresh backup, retain all
   business IDs, identity links, grants, invoice/payment history and ciphertext
   references, and compare pre/post records. Do not reset or touch business_prod.
8. Release backend and website together, verify hosted login and tenant isolation,
   then delete the remaining PortalService, routes, hooks and obsolete callers.
   Preserve historical audit records and useful migration/import tools.

Checks run at this checkpoint are recorded in the change description. Passing a
subset does not mean all dead code is removed or hosted customer flows are accepted.
