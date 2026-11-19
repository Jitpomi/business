# Generic background execution and scheduling

Dog-queue owns worker pools, discovery, recovery, graceful draining, delayed
execution, retry timing, recurring occurrence generation and catch-up. Business
contains storage adapters and domain job payloads, not timer loops or recurrence
arithmetic. The existing in-memory cron API is not used.

The TypeDB adapter implements RecurrenceBackend. In one revision-fenced transaction
it advances the saved cursor, stores the occurrence delivery and publishes the next
scheduler wake-up. A crash or lost acknowledgement cannot publish the same sequence
twice. Fixed-rate catch-up preserves every occurrence; nothing is silently skipped.
Schedule IDs, next timestamps, sequence counters and legacy occurrence keys remain
compatible. Existing queued schedule references call the same generic dispatcher;
there is no second scheduling algorithm or fallback poller.

Scheduled<TriggerWorkflow> dispatches business work using the original
schedule:<id>:<sequence> key. It rechecks schedule state and the existing native
workflow-owner/connection rules. Native authorization denials can pause business
work; infrastructure failures are retained for retry and are not classified as
permission revocation. Occurrence jobs remain inspectable even when paused or
failed. After review, resubmitting the original workflow/input with its recorded
occurrence key preserves business idempotency. An uncertain payment still requires
evidence-based resolution through the existing journal; it must never be resent.

Queue records and scheduler metadata have independent native fences and do not
modify tenant business revisions. TypeDB remains authoritative for tenant RBAC,
consent and financial claims. Lease tokens are omitted from owner inspection.
There is no production memory queue, additional storage service or schema reset.

Before deployment, quiesce development writers, export and restore-verify a fresh
backup, and rehearse backfill in an exact restored shadow. Verify original data,
attributes and relation roles. Backfill changes no existing business records and
calls no providers. Keep original snapshots and all uncertain-outcome journals.
Rolling versions share workflow CAS and stable occurrence keys. New scheduled job
payloads remain durable if an older binary does not support their handler; never
remove or rewrite them to force rollback. Restore a supporting binary to drain them.

The service starts one dog-queue worker per tenant, with four global execution
slots across tenants. Native business writes share a tenant revision fence;
concurrent cycles for the same tenant must not compete with each other merely
because several occurrences became due during downtime. Catch-up occurrences are
retained and drained in order, not coalesced or skipped. Native revision/lease
checks and financial journals still protect races with requests or other service
instances; this worker configuration is not a replacement for those checks.
