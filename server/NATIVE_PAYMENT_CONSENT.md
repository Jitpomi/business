# Native automatic-payment consent

The buyer creates one immutable `bill_consent_evidence` graph through `/data`.
That single TypeDB transaction inserts the grant, its buyer/customer ownership and
its approved-plan scope links. No mutable application flag activates payments.
The model validates buyer ownership, exact billing revision, selected plugin and
method, provider-attested automatic-payment capability, active/non-pending method,
fresh verification, active approved plans and the authenticated request time.

`consent_claim = customer_id + ":" + billing_revision` is unique. A grant cannot
be duplicated, edited, deleted or extended to another plan after commit through
the public API. Seller preferences cannot create buyer consent. Plans have
immutable price/cadence data; every plan being approved must be explicitly linked
in the transaction. A later or unlisted plan remains manual.

## Buyer flow

1. Use `/financial` with `action: payment_methods`, the local customer ID and the
   merchant's selected plugin alias. Native policies authorize the buyer before
   the tenant plugin is resolved. The plugin verifies provider facts; clients
   cannot insert saved-method attestations.
2. Submit `billing-select-plugin.tql` and `billing-select-method.tql` through native
   writes. These choices alone grant no collection authority.
3. Show the buyer the exact immutable plans, amounts, cadence and billing revision.
   Render `billing-grant-consent.tql` using those approved values and a stable
   request UUID. `request_identity()` and `request_now()` come from authenticated
   context, not client IDs or a browser clock. Add a scope relation for every
   additional approved plan in that same insertion.
4. On a timeout or concurrent conflict, read the grant by its request ID and
   customer/revision claim before resubmitting. Do not invent a new request ID or
   silently substitute a newer billing revision or different payment method.

The canonical `bill_has_consent` and `bill_plan_has_consent` functions are the
collection authority. The worker consumes their result. Invoice issuance snapshots
its billing revision; collection checks the current grant for that exact plan and
revision again in the atomic attempt claim. Provider calls stay outside database
transactions and retain their original idempotency keys. Revocation committed
before an attempt claim prevents collection; a provider request already claimed
and in flight must be reconciled, not assumed cancelled.

## Revocation and changed terms

`billing-stop-automatic.tql` changes the customer billing revision to a new unique
request UUID. That single native update immediately invalidates the old grant and
pending invoice authorizations. TypeDB rejects restoring any revision that already
has consent history. A new grant requires fresh buyer approval and provider method
verification; it cannot authorize an invoice issued under the revoked revision.

Legacy customer `bill_choice`/`bill_consent` and evidence `bill_updated` values are
retained as read-only history for preservation. They have no runtime authority.
Native consent uses server-authenticated `consent_at`; new grants need no client
clock or legacy flag. There is one active authorization path.

## Explicit development migration

Stop development writers, export a fresh backup, restore-verify it, then rehearse:

```sh
BUSINESS_ENV_FILE=/private/dev.env \
BUSINESS_BACKUP_DIR=/private/verified-backup \
cargo run --locked --bin migrate_consent -- --rehearse business_native_dev_v2
```

The migration imports into a new generated shadow, compares the complete logical
graph (objects, all attributes and relation roles), reviews unknown schema names,
and retains the source and backup. It never invents consent from old flags. An
active legacy flag without a verifiable native grant stops cutover and identifies
customers needing buyer re-consent; their records and failed shadow remain intact.

After rehearsal and provider acceptance, with writers still stopped and the backup
still equal to the source, use `--apply` with `BUSINESS_WRITERS_STOPPED=true` to
create `business_native_dev_v3`. No existing target is overwritten. Deploy matching
code/configuration only after verification; keep the original source and runtime
configuration for rollback. Credentials, Auth0 subjects and encrypted connection
references are preserved. Startup never migrates a schema or resets data.

## Acceptance

See `STRIPE_SANDBOX_ACCEPTANCE.md`. The real Stripe test flow now uses distinct
seller and buyer identities, public financial attestation and native consent.
It covers successful/declined payment, explicit plan scope, revocation, renewal,
old-revision replay rejection, concurrent submission, rollback of forbidden grants,
independent tenant notification plugins and restart without duplicate effects.

Customer-facing consent status uses `request_bill_payment_choice`, a scoped TypeDB
context function. It exposes the derived status to authorized buyers and sellers
without exposing the buyer-only provider-method facts used to calculate it.
