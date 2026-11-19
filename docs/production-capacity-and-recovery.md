# Production capacity and recovery — 19 November 2026

Status: implementation prepared; paid infrastructure not purchased or deployed.
Real customer payments remain disabled in the existing checkout implementation.
Development remains on its original service, database and encrypted stores. This
file does not waive the three dependency advisories or authorize live activation.

## Initial capacity and cost

| Component | Prepared configuration | Observed quote |
| --- | --- | --- |
| Application | Separate `jitpomi-business-production`, Render Virginia, one always-on 1 CPU / 2 GB instance, manual deployments, 300-second shutdown allowance | $25/month |
| TypeDB | Separate GCP US East `jitpomi-business-production`, TypeDB 3.15.0, one 2-burstable-vCPU / 4 GB server, 10 GB storage, hourly backups retained seven days | $0.07/hour, about $51.10 per 730-hour month |
| Secrets and monitoring | Separate production D1 databases/workers, keys, canary and access tokens; existing provider implementations | Usage charges depend on the account/plan; not included above |

The combined compute estimate is $76.10/month at 730 hours, before taxes,
usage, backup/storage extras, GitHub runner/artifact charges or IdP/provider fees.
The TypeDB account currently has no payment method. No charge was submitted.
The quotes are dashboard observations, not a fixed-price commitment.

This is a deliberately modest starting capacity, **not high availability**.
One app instance and one database node can be unavailable during a host failure.
The dashboard's three-node TypeDB Medium preset was $0.49/hour; do not silently
purchase it or claim a single node has automatic multi-node failover. Keep manual
scaling until a paid-capacity load test establishes the safe operating envelope.

`deploy/production/render.yaml` is a separate opt-in Blueprint; root `render.yaml`
continues to describe development. The worker configurations explicitly require
new production D1 IDs and fail deployment until those IDs exist. They do not reuse
or rename development stores. No database is created/reset at application startup.

Initial resource budgets remain three active HTTP requests, 32 bounded waiting
requests and four simultaneous background executions per process; one queue worker
per tenant. Admission protects resources, never grants permission. Native TypeDB
membership, scope, prices, consent, claims and effect journals remain authoritative.
Do not increase these budgets merely to hide timeouts or STC2 contention.

### Capacity acceptance before routing production users

Run the synthetic authenticated native-read test against a newly generated test
namespace on the paid cluster, using operator credentials only for fixture setup.
It asserts exact own-resource visibility for two tenants on every request; it never
loads a payment plugin. Run the complete checkout gateway suite against generated
fixtures as well. Use sandbox plugins exclusively for provider acceptance.

Required paid-environment soak: 30 minutes at three concurrent representative
requests, then a bounded burst of ten clients. Record the dataset size, exact
commit, instance/cluster specs, request count, accepted/error/overload counts,
p50/p95/p99 latency, peak CPU/RSS and background lag. Initial targets: p95 <= five
seconds, p99 <= fifteen seconds, <1% unexpected errors, no tenant leakage, no
lost journal facts and no duplicate provider effects. These are initial targets,
not measured production throughput. Health checks alone do not establish capacity.
An admission rejection must occur before the business handler starts.

Review capacity when CPU or memory stays above 70% for 15 minutes, disk reaches
70%, p95 misses five seconds, or due work lags more than five minutes. At 85% disk,
stop onboarding and enlarge storage before it fills. Alert on overdue work at the
existing fifteen-minute threshold. Scale and rerun the same acceptance; do not
relax native policies, skip schedules or retry uncertain money movements.

The local synthetic rehearsal on TypeDB 3.14.1 completed 120 requests at concurrency
three in 23.66 seconds: p95 959 ms, p99 995 ms, exact tenant isolation, zero provider
calls. This is a test of the workload/tool and native boundaries, not certification
of the proposed paid hardware or of a 30-minute production soak. The same test on local TypeDB 3.15.0
completed in 23.54 seconds, p95 949 ms and p99 983 ms, with the same isolation
assertions and no provider calls.

## Recovery objectives

| Objective | Initial operational target | Measurement / enforcement |
| --- | --- | --- |
| Availability | 99.5% over a rolling 30 days | Count service/database dependency failures and customer-facing unexpected 5xx; planned outages count. This is an internal objective, not a vendor SLA. |
| RPO | At most two hours for graph + credential/config state | Hourly encrypted offsite checkpoints and hourly native database backups; production monitor alerts when a verified offsite checkpoint is 90 minutes old. |
| RTO | At most four hours from incident declaration to authenticated, verified service restoration | Include obtaining the key, fresh infrastructure, full graph/ciphertext/config restore, authentication, RBAC and read-only provider reconciliation. Import time alone is insufficient. |
| Detection / response | Monitor every five minutes; operator acknowledgement target 30 minutes | Two failed samples open an incident; independent watchdog checks heartbeat/outbox. Scheduler/mail delays remain possible and must be included in incident timing. |
| Rehearsal | Monthly and after schema/key/topology changes | Download an offsite artifact, use the separately held password-manager identity, restore isolated targets and retain stage timings plus assertions. |
| Retention | Seven days native snapshots; thirty days recipient-encrypted offsite artifacts | Retain the separate verified Drive recovery checkpoint; storage/runner retention and provider account access are separate dependencies. |

The two-hour RPO includes orders, access changes and automation journals; provider
reconciliation alone cannot reconstruct a lost order or a revoked membership.
Hourly GitHub scheduling is not a timing guarantee. A checkpoint failure must alert
immediately; a 90-minute age breach is an incident, and passing two hours means
**the target was missed**, not that the recovery point moved forward. Stop new
commerce acceptance/onboarding when the target is breached; preserve current
processing and reconcile it. No automatic replay of a financial action is implied.
A missed target stays visible until an acknowledged, verified fresh artifact exists.

The production backup caller is disabled until `BUSINESS_PRODUCTION_BACKUPS_ENABLED`
is explicitly set to `true`. It reuses the audited checkpoint/rehearsal/encryption
implementation, with separate production secrets and a fixed production profile:
`business_prod`, `business_prod_backup`, verified TLS and production-only snapshot
endpoints. A development credential/source/receipt cannot satisfy production.
The backup runner never receives the recovery identity or decrypted master key.

Production monitor requires readiness v2, all nine aggregates, its own backup
journal and `BACKUP_MAX_AGE_SECONDS=5400`. Production alerts/watchdog identities are
separate from development. Both alert paths still share the mail provider and
operations mailbox: verify an independent human escalation/contact route before
claiming around-the-clock response. Do not infer an on-call commitment from an
email address. Alerts remain directed to dev@jitpomi.com.

## Provisioning and acceptance order

1. Approve recurring spend and add billing details in TypeDB personally. Create the
   separate cluster with the capacity/region/version and hourly/seven-day settings
   above. Keep `dogrs-validation` and every development database intact.
2. Install the canonical schema into a **new** `business_prod` with explicit
   provisioning, not startup migration. Create distinct least-privilege runtime
   and backup accounts. Runtime must not have schema/user management or reset
   privileges; verify rejected schema writes. Retain operator access separately.
3. Create the two new production D1 databases, apply their canonical schemas and
   replace only the corresponding placeholder IDs. Deploy the shared secret and
   monitor workers with distinct strong tokens. Create a new production vault key,
   preserve its separately encrypted configuration capsule, and verify key custody
   in the password manager. Do not copy development tenant/plugin accounts.
4. Configure a separate IdP application/audience and exact origins for production;
   use the existing dog-auth plugin boundary. No passwords enter TypeDB. Configure
   the opt-in Blueprint with the new database, keys and canary. No live Stripe key
   or live merchant capability is installed. Keep public account traffic on
   development until this environment passes its own acceptance.
5. Set the prefixed production backup secrets listed in the caller workflow and
   the production recipient variable. Manually dispatch and download its first
   successful artifact. Verify full graph and every referenced ciphertext/config
   key before enabling the hourly production switch. The daily development job
   remains enabled and unchanged in cadence.
6. Start the recovered app on a new generated shadow with
   `BUSINESS_RECOVERY_MODE=true`, an independent ciphertext store and no workers.
   Measure the full RTO from declaration, including provisioning/key retrieval.
   Verify fresh login, own/foreign reads, revocation, rejected mutations/webhooks,
   all nine readiness metrics, wrong-key/tenant rejection, graph-role equality and
   independent provider observations. Keep uncertain external effects withheld.
7. For a real disaster, prefer a native backup clone to a **new cluster**, retaining
   `business_prod`. If only an offsite export survives, first validate its complete
   graph in the generated read-only shadow; explicitly import into a fresh empty
   production cluster and verify again. Existing targets must never be overwritten.
   Reconcile from the checkpoint time to the current provider state before any
   worker is released. Retain original and restored evidence and unknown outcomes.
8. Complete paid-capacity soak, restart/drain/recovery tests, real alert delivery,
   independent key retrieval, and operator escalation acknowledgement. Enable the
   prefixed production monitor/watchdog switches only after verified checks. Record
   actual RPO/RTO, not an estimate. Keep payments disabled throughout this pass.

This deployment work is still blocked on paid provisioning/billing and its fresh
infrastructure acceptance. The existing encrypted recovery rehearsal proves the
current development state is recoverable; it does not close the production RTO
or offsite-backup gate for an uncreated environment. Remaining dependency findings
and live merchant/money acceptance are separately tracked release blockers.

Sources checked: [Render compute plans](https://render.com/docs/compute-plans),
[Blueprint fields](https://render.com/docs/blueprint-spec), and
[TypeDB backup/clone support](https://typedb.com/docs/maintenance-operation/typedb-backups/).
