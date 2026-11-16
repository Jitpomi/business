# Background jobs

Business uses dog-queue for typed dispatch, leases, heartbeats, retries and graceful
draining. TypeDBBackend persists the queue in the private automation schema. No
production memory queue or additional database is used.

Workflow and schedule changes insert a revision delivery in the same transaction.
Advance reloads current workflow state and ignores obsolete revisions. Delay and
retry timestamps become queue eligibility times. Approval, blocked, attention and
terminal states create no runnable delivery until an authorized transition occurs.

The supervisor discovers tenant namespaces in pages, backfills missing deliveries
once per namespace, and starts four workers per tenant. A shared semaphore bounds
active handlers to four across tenants. It never executes steps or scans for due
business records. Failed pools are drained and restarted. All pools drain together
on shutdown; await Worker::shutdown before backing up or deleting a database.

Queue operations are tenant-bound, revision-fenced TypeDB transactions. Job metadata
uses its own fence and never changes the tenant business revision. Competing claims
and uncertain metadata commits are reconciled by exact job state and lease ownership;
this never retries a handler or provider operation. Lease tokens
and deadlines protect acknowledgements. Native schema policies deny raw access;
owner operations use TypeDB RBAC functions. The workflow rechecks its owner and
atomically claims the approved connection before external effects. Queue possession
does not authorize payment.

The existing workflow journal remains authoritative for consent, approvals, provider
idempotency and uncertain outcomes. Redelivery never means blindly resending a
payment. Interrupted at-most-once attempts require evidence-based reconciliation.
Failed deliveries remain persisted and available in owner-authorized job summaries
and inspection, with lease tokens omitted. There is no automatic deletion or reset.
The backend provides durable status reads, not live events or priority ordering.

## Cutover

No schema migration is needed: jobs use a new kind of private automation record.
Stop old writers, export and restore-verify a fresh backup, rehearse backfill on a
restored copy and verify existing business records remain unchanged. Backfill only
adds missing revision deliveries and makes no provider calls. Keep original backups.
Quiesce writers for backup and rehearsal. During a rolling replacement, both
versions use the same native workflow CAS and effect journal; the earlier poller
ignores job records. Queue backfill changes no existing business attributes.
Rollback uses the earlier binary; quiesce new workers first.

The dog-queue dependency pins reviewed upstream source with graceful drain and pool
health support. There is no copied worker or local path dependency, and no new
public package release is needed.
