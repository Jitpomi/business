# Identity and authentication

A person is a human, independently of whether they can sign in. They have a stable
`identity_id`, name and lifecycle status. Email is optional contact data; changing
it does not change a login identifier. Memberships and access grants refer to the
identity, never to a credential.

`authentication_credential` is an abstract relationship to exactly one person. A
person may have zero or many credentials. Each credential has its own globally
unique `credential_id` and `credential_version`. `password_credential` implements
local password authentication with a unique `login_email` and private hash.
Additional authentication strategies can introduce credential subtypes without
changing the person or their memberships. This change implements local passwords;
it does not claim to implement OAuth, passkeys or account-linking UI.

Local sign-in resolves a login address to its credential and person, verifies the
password, and checks active tenant membership. JWTs identify both the person and
the credential. Each request rechecks the credential's ownership/version and the
person's active membership. Rotating/removing one credential invalidates that
credential's sessions without affecting another credential. Person suspension or
membership termination still applies across all credentials.

Credential records, identifiers, login addresses and hashes are not available to
ordinary `/data` queries. They are managed through trusted authentication paths.
Never link identities just because an external provider supplies the same email;
a future linking flow must prove control of both accounts.

Invitation acceptance reuses an existing credential after password verification,
or attaches the first password credential to an existing contact after validating
the one-use invitation. It does not replace the person's ID. Password recovery
binds the one-use token to a specific credential and its version, not every login
belonging to that person.

## Existing database migration

The application never migrates on startup. New schema code deliberately rejects
an unmigrated database. Keep the existing deployed binary running until a planned
maintenance cutover; do not deploy the new binary against the old schema.

1. Stop all HTTP writers and background workers for the target.
2. Export schema, data and record evidence with `database_backup export`. Run
   `restore-verify` and retain that verified backup privately.
3. Use an explicit operator environment for the target, with
   `BUSINESS_WRITERS_STOPPED=true` and `BUSINESS_BACKUP_DIR` pointing to that backup.
4. Run `cargo run --locked --bin migrate_credentials -- --apply`. The command checks
   that the backed-up record evidence still matches the database before changing it.
5. Verify people, memberships and business records against the backup. Password
   hashes, versions and login addresses move unchanged into credential relations.
   Existing reset links retain their targets because the initial credential ID is
   the original person ID; newly created credentials use independent UUIDs.
6. Deploy the matching binary, test sign-in, tenant access, recovery and invitation
   acceptance, then resume workers.

The migration is repeatable and refuses conflicting existing credentials. It
copies and checks values before removing credential ownership from people; it
never resets the database or hashes an existing password again. Schema changes
span transactions, so all writers must stay stopped until completion. If a stage
fails, retain the backup and rerun against the stopped target after inspection.

Existing JWTs do not contain a credential ID and deliberately require signing in
again. Passwords and identities remain unchanged. Before reopening writes, rollback
can restore the verified backup with the matching old binary. After writes resume,
do not overwrite new work with an older snapshot.

## Verification

Run ordinary Cargo/Node checks, then the real TypeDB backend suite using development
operator/runtime environment files and generated disposable databases. The
`identity_tests` cover no-login people, multiple credentials, independent sessions,
credential privacy and migration preservation/repeatability. The billing account
acceptance covers invitation reuse, recovery races and session invalidation without
external provider calls or email.
