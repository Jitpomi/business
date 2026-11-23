# Tenant checkout configuration

Status: implemented and verified in an isolated TypeDB 3.14.1 environment. Development cutover is still pending; real checkout payments remain disabled.

## Authority and history

Configuration versions and notification templates are immutable TypeDB records. A merchant has one current configuration head. Publishing or selecting a version requires a native owner/delegated configuration permission and an immutable authorization record in the same transaction. The gateway supplies a compare-and-set expected head to reject concurrent edits.

New quotes pin the current merchant configuration. TypeDB checks permitted currencies, offer kinds, payment modes, cart lines, schedule groups, quantities and review expiry. A changed head invalidates unaccepted quotes. Acceptance never grants automatic collection consent. Already accepted orders retain their original configuration, including refund policy and notification templates. Buyer cancellation/revocation is always available under the existing native policies.

Existing orders without a configuration snapshot retain the original checkout behavior. They are not silently backfilled with terms the customer never reviewed. New quotes cannot omit their configuration once a merchant has published one.

The configuration editor is `/account/checkout-settings.html`. The canonical field contract is `server/public/checkout-configuration.json`; the website contains its versioned copy. Run `python3 deploy/checkout/generate_schema.py --check` to verify the generated native fields. Handwritten policies remain in `server/src/commerce-configuration.tql`.

## Hardcoded-settings audit

| Setting | Owner and enforcement |
| --- | --- |
| Brand, tagline, heading, introduction, help, footer, button wording | Tenant version; plain text rendering |
| Logo, home, support and terms links | Tenant version; HTTPS only, no embedded credentials |
| Accent, foreground, background, font, radius and layout | Tenant version; constrained presentation tokens, no executable CSS/HTML/JS |
| Locale and notification time zone | Tenant version; money formatting in browser, IANA zone rendering using bundled Jiff data; UTC fallback for unknown raw-client zone names |
| Allowed currencies, modes and offer kinds | Tenant version; native TypeDB creation policies |
| Cart line, quantity and schedule-group limits | Tenant version within platform bounds; native TypeDB policies |
| Review expiry | Tenant version; native TypeDB acceptance policy |
| Refund availability/window | Accepted order's version; native TypeDB refund-request policy, provider capability required |
| Notification subject, text, enablement, interval and occurrence count | Accepted order's version; native delivery decisions and existing durable claims |
| Invoice payment window (0–90 days) | Accepted order's version; native projection into invoice due-date calculation |
| Invoice display name | Accepted order's version; native query projection into billing execution |
| Default currency, instalment count, widget mode, button/title and inline payment theme/layout | Tenant version; presentation defaults, not permission or provider capability grants |
| Tax, discount, shipping, donation bounds, instalment periods/deposit/count, public buyer onboarding | Existing immutable native offer/pricing versions; no duplicate price calculator |
| Financial plugin and notification connection | Tenant-owned verified plugin configuration; existing native owner/store guards |
| Schedule ID, workflow ID, connection ID, enablement and interval | Existing dog-queue scheduling; native automation limits; retain custom workflow steps and recurrence history |
| Provider credentials/OAuth and sender identity | Tenant plugin configuration; encrypted external credential store, not commerce/identity graph |
| Settlement, exactly-once claims, consent, cancellation, sandbox labeling, real-payment block | Platform safeguards, never tenant presentation options |
| Supported currencies/capabilities and external provider limits | Plugin declarations; tenant settings cannot invent support |
| Transport/query/resource ceilings | Platform bounds; tenant limits may narrow them |

Refund windows are measured from order creation, explicitly rather than an invoice's mutable reconciliation timestamp. Template variables are allowlisted; unknown placeholders are rejected. The verified amount, invoice identity and factual payment message remain attached to tenant copy, and sandbox subjects retain their test prefix. An uncertain notification or refund is not automatically resubmitted.

The current data proxy limits a query to 32 KiB. The editor must reject oversized combined configurations clearly; increasing this framework safety limit is not part of customization.

## Verification and cutover

Tests must cover both merchants' independent settings, foreign edits/reads, direct `/data` bypass attempts, stale quotes, immutable accepted snapshots, audited version selection, scheduling preservation, plugin capability checks, actual gateway authentication, queue execution, payments, partial refunds, notifications and uncertain-effect recovery. Existing orders and legacy configuration paths need regression coverage.

The checkout schema release is v11. The physical development database remains `business_native_dev_v10`: a database resource name is independent of its schema version. The Render manifest retains that existing resource. Runtime startup verifies the required schema; it never renames a resource or migrates automatically.

`migrate_checkout --apply-schema SOURCE PRIVATE_CHECKPOINT_DIRECTORY` performs an explicit, additive update in one TypeDB schema transaction. It first restores and rehearses the complete checkpoint on the loopback verifier using the compiled target schema, compares all original facts and relationship roles, and verifies prior automatic-payment approvals. A TypeQL AST comparison rejects removed definitions, changed type annotations/capabilities, and changed public function signatures. Only new definitions/capabilities and reviewed function-body replacements are eligible.

The exclusive schema transaction blocks concurrent writes while the source schema and complete checkpoint are checked again. Every schema query shares that transaction; a failed query closes it without commit. A lost acknowledgement can be checked by rerunning: only the exact source or target schema with the unchanged checkpoint is accepted. Each attempt retains its plan, local recovery shadows and completion evidence. No payment, consent or provider action is replayed.

Before development cutover: quiesce writers, take a fresh checkpoint and offsite encrypted recovery archive, restore and compare the complete graph and prior approval decisions in an isolated verifier, apply the atomic schema update to the existing resource, update the active runtime/backup revision together, then deploy and verify sandbox-only flows. Keep the original checkpoints and recovery shadows. Runtime startup verifies the new schema and performs no implicit migration or reset.

Verified locally:

- Actual website gateway, dog-auth, native TypeDB policies, dog-queue and synthetic tenant plugins: full checkout/payment/partial-refund/receipt/uncertain-effect suite passed (557.87 seconds). Configured templates retain the accepted brand and Africa/Kampala time zone after publication changes.
- Direct data-layer ownership, delegated permission/revocation, cross-tenant isolation, immutable snapshots, audited activation, current-head enforcement, currencies/modes/offer kinds, line/group/quantity limits, quote clock bounds and refund/receipt windows passed. A rejected refund leaves no execution proposal.
- Operational metrics and complete graph recovery passed. The downloaded encrypted development checkpoint restored all 78,719 objects with their attributes, roles and prior approval decisions unchanged.
- Website: 75 tests passed; Cloudflare packaging passed a dry run. Rust all-target tests and Clippy with warnings denied passed.

The audit also updated maintained upstream `url` to 2.5.9 for RUSTSEC-2026-0592. The three existing TypeDB maintenance advisories remain visible in the separate dependency-security workflow.

An equivalent literal-status predicate reduced the overdue-job query from 30,688 ms to 1,264 ms against the same read snapshot and count (340), avoiding the readiness deadline without changing thresholds or hiding history. Completed synthetic checkout schedules were paused through their owners' native controls, preserving cursor and execution history. Existing uncertain runs were not replayed or marked successful.

Deployment acceptance still requires the coordinated checkpoint, migration and hosted checks above. No live-payment activation is authorized by this change.


Transactional checkpoint manifests come from an isolated restore. Internal TypeDB storage IDs can change during that import, so the migration source guard compares the complete multiset of record attributes and the logical graph (including every relationship role and multiplicity). The binary must also reproduce the relationship manifest before a candidate is imported. New work or changed ownership still rejects the cutover before source retirement.

The atomic-update operator was independently rehearsed against all 79,246 objects from the retained v10 checkpoint on local TypeDB 3.15.0. Target schema, all original attributes, complete relationship roles and existing approval decisions matched; closing an uncommitted full update left the original schema intact. This is local evidence, not a claim that development has been upgraded.
