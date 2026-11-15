# Development plugin migration — 2026-11-15

Target: `business_native_dev`. `business_dev` (the older model) and production were not modified.

The existing Render writer was identified under the `dev@jitpomi.com` account,
confirmed to use the canonical development target and sandbox mode, then suspended.
The service returned HTTP 503 before the final backup and migration.

Schema, TypeDB binary data, model counts, all entity/relation attribute records,
and the previous hosted environment were backed up privately with restricted file
permissions. The TypeDB backup was restored into a newly generated disposable
database, its model counts matched, and that exact verification database was removed.
Checksums are stored with the private backup; credentials and backup data are not in Git.

The explicit migration copied all 15 customer binding maps. Post-migration validation
confirmed all 127 existing entity/relation IDs, their original attributes and every
model count were preserved. This includes 15 accounts/customers, 6 plans, 4 invoices,
5 notices, 3 account links, 11 people and 16 memberships. New required plugin fields
and retained optional legacy references were verified. Existing plans were paused
or finished; historical invoices and claims were not reset or retried.

Hosted configuration was converted to tenant financial/notification settings, explicit
worker tenants, an account route alias and the canonical gateway-secret name. Existing
credentials were retained within the same tenant. The previous values are recoverable
from the private export. Empty Mercury proxy configuration became an empty array.
Render's repository was updated from the old `jermsam/business` path to public
`Jitpomi/business`, branch `main`, with automatic deployment still off.

## Sandbox evidence

Stripe's full real sandbox acceptance test passed (94.74 seconds). It verified
scheduled invoices, a successful payment and accepted receipt, a declined payment
and accepted reminder, repeated ticks without duplicate effects, webhook checks,
authenticated portal operations, invitations, recovery and session revocation.
Only synthetic test records and the approved validation mailbox were used. Email
provider acceptance does not prove inbox delivery.

Mercury's registered service checks passed account/customer/invoice list and detail
reads. Transactions were denied; a direct baseline read reported an IP restriction.
The first billing-plugin acceptance test failed closed at destination-account
preflight with HTTP 401, before creating a database, customer or invoice. After
sandbox sign-in, the browser and test runner were found to use different egress
IPs. The user approved adding only the exact test IP while retaining the existing
allowlist entry. No token permissions were expanded.

The next run passed preflight, created a synthetic USD 1 customer/invoice through
the Mercury plugin with no Stripe connection, reconciled the provider response,
and accepted one reminder while suppressing its replay. The hosted invoice and PDF
were verified. The hosted payment form is ready for user submission; payment/receipt
certification remains pending. The generated test graph was briefly quiesced,
exported, restored into a separate disposable database and count-verified, then the
verification database was removed. This private checkpoint preserves the pending
invoice and notice bindings if the interactive payment takes longer than the test
wait window. Existing development and production records were not changed.

Mercury card and ACH debit options now default to false. Card acceptance can be
explicitly enabled for a tenant that has connected Stripe within Mercury; basic
bank-transfer invoicing does not require that connection.

## Backup operator commands

The development-only `database_backup` executable supports `inspect`, `export`,
`restore-verify` and `verify-preserved`. Pass an explicit `business_native_dev` or
`business_dev` target, or an explicitly named `business_backend_test_<UUID>`
checkpoint. Export to a pre-created private directory under `umask 077`.
It refuses an existing manifest and never overwrites a database. Restore verification
uses a new UUID target and deletes only that generated target. Preservation verification
checks existing IDs, original attribute values, model counts and upgraded schema.
Pause all writers before using a backup for migration or preservation comparison.

```sh
BUSINESS_ENV_FILE=/private/operator.env cargo run --locked --bin database_backup -- export business_native_dev /private/backup-directory
BUSINESS_ENV_FILE=/private/operator.env cargo run --locked --bin database_backup -- restore-verify business_native_dev /private/backup-directory
BUSINESS_ENV_FILE=/private/operator.env TYPEDB_DB=business_native_dev cargo run --locked --bin migrate_financial_plugins -- --apply
BUSINESS_ENV_FILE=/private/operator.env cargo run --locked --bin database_backup -- verify-preserved business_native_dev /private/backup-directory
```
