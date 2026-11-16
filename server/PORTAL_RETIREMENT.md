# Portal retirement

The backend `/portal` service, route, hooks and duplicate commands are removed.
The customer website remains at `/account/`.

| Responsibility | Boundary |
| --- | --- |
| Profile and company discovery | Frontend TypeQL, native context functions |
| Customer/buyer creation and invitation lifecycle | TypeDB schema and policies |
| Verified payment-method facts | Financial plugin bridge only |
| Default choice, consent, removal intent | Atomic TypeDB transactions |
| Confirmation and customer-bound detach | Tenant financial plugin |
| Credential challenges, account linking/acceptance, recovery | Authentication plugins |
| Reconciliation | Existing durable automation engine |

Permissions use the authenticated identity and tenant with server-owned policy
time. Discovering a person's companies does not broaden `scope_tenant` for graph
access. A company can both sell and buy; customer records expose the native buyer
and seller checks so the website can show both sets of actions.

Automatic payments require a provider-verified active method, native ownership,
applicable policy, active schedule and fresh explicit consent evidence. Stale
revisions cannot commit. Saving or confirming a method does not authorize a
charge. Removal intent commits before a provider call; a failed acknowledgement
leaves removal pending and does not restore automatic payments. Detach is bound
to the expected external customer and uses a stable retry key.

## Migration and rollback

Use the existing free infrastructure. The explicit `migrate_portal` tool imports
into a new development shadow database; it never resets or mutates the source.
Existing business IDs, attributes, issuer/subject links, grants, invoice/payment
history, workflow/run state and external ciphertext references are preserved.
TypeDB remaps internal storage IIDs during import. Preservation is therefore
verified using a full logical graph, including relation role bindings and record
multiplicity, not by equating storage IIDs with application identity IDs.

Historical invitation integer expiries are retained. The tool adds their exact
UTC datetime equivalent without renewing an expired invitation. Existing consent
evidence is retained without inventing timestamps. New consent uses native
`consent_at`; verified-method facts are not fabricated from old provider IDs.
Custom schema definitions absent from the canonical model block migration for
preservation review instead of being silently dropped.

1. Rehearse on an existing restore-verified backup with `--rehearse`.
2. Stop the development service and confirm its writers have stopped.
3. Export and restore-verify a fresh private backup.
4. With `BUSINESS_WRITERS_STOPPED=true` and the matching backup directory, run
   `migrate_portal --apply business_native_dev`. The new target is
   `business_native_dev_v2`; an existing target is never overwritten.
5. Keep the source database, original encryption key and durable secret store.
6. Verify imported connection decryption and non-admin runtime access before
   changing Render's database target and deploying the tested backend commit.
7. Deploy the matching website Worker and verify hosted Auth0 login, acceptance,
   native tenant isolation, payment settings and the retired route's HTTP 404.

Rollback uses the retained source database with the previous backend and website
releases. Do not roll back after accepting new writes without reconciling those
writes first. Neither startup nor rollback instructions perform a destructive
reset or credential rotation.

## Verification

The checks cover the actual website TypeQL against disposable TypeDB databases,
independent bank/mail plugins and the existing background worker, including
invitations, replay/expiry, default choice, consent, collection, reconciliation,
failed detach and explicit retry. Existing recovery/reset races, identity linking,
multiple authentication providers, machine identities and immediate revocation
remain covered. Stripe TEST payment/receipt and decline/reminder checks exercise
the built-in provider boundary. Hosted deployment acceptance is recorded separately
from local and disposable-database test results.
