//! Durable scheduling and reconciliation. External requests never run inside a DB transaction.
use super::billing_schema::{self as schema, Mode};
use crate::{
    finance::{opaque_id, InvoiceObservation, InvoiceRequest, TenantRuntime},
    typedb::TypeDBState,
};
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct Engine {
    pub state: Arc<TypeDBState>,
    pub merchant: String,
    pub runtime: Arc<TenantRuntime>,
    permission: Option<super::access::Permission>,
}
pub const CUSTOMER_DATA: &str = r#""customer_id":$c.bill_key,"name":$c.name,"email":$c.bill_email,"policy":$c.bill_policy,"choice":bill_payment_choice($c),"manual_provider":$c.bill_provider,"bindings":$c.bill_bindings,"method_plugin":$c.bill_method_plugin,"payment_method":$c.bill_payment_method,"consent":bill_approved_revision($c),"revision":$c.bill_revision"#;
impl Engine {
    pub fn new(
        state: Arc<TypeDBState>,
        merchant: &str,
        runtime: Arc<TenantRuntime>,
    ) -> Result<Self> {
        let merchant = schema::id(merchant)?;
        ensure!(runtime.tenant == merchant, "Tenant plugin scope mismatch");
        Ok(Self {
            state,
            merchant,
            runtime,
            permission: None,
        })
    }
    pub fn binding(customer: &Value, plugin: &str) -> Result<String> {
        let bindings: std::collections::HashMap<String, String> =
            serde_json::from_str(string(customer, "bindings")?)?;
        Ok(bindings.get(plugin).cloned().unwrap_or_default())
    }
    pub fn method_plugin(&self, customer: &Value) -> Result<String> {
        let selected = string(customer, "method_plugin")?;
        if !selected.is_empty() {
            return Ok(selected.into());
        }
        self.runtime.default_financial.clone().ok_or_else(|| {
            dog_core::DogError::conflict(
                "Payment setup is unavailable. Ask this company to connect a financial system.",
            )
            .into_anyhow()
        })
    }
    /// Fixed trusted scope, repeated in every native read and write. It cannot be
    /// replaced by an unverified request field or a cached Rust role decision.
    pub(crate) fn with_permission(mut self, permission: super::access::Permission) -> Self {
        self.permission = Some(permission);
        self
    }
    pub fn prefix(&self) -> String {
        format!(
            r#"match $t isa tenant,has tenant_id "{}",has status "active",has revision $revision;{}"#,
            self.merchant,
            self.permission
                .as_ref()
                .map(|p| p.clause())
                .unwrap_or_default()
        )
    }
    pub(crate) async fn require_access(&self) -> Result<()> {
        let rows = self
            .read(format!(
                "{} fetch {{'tenant':$t.tenant_id}};",
                self.prefix()
            ))
            .await?;
        ensure!(
            rows.len() == 1,
            dog_core::DogError::forbidden("You do not have access to this billing account")
                .into_anyhow()
        );
        Ok(())
    }
    pub async fn read(&self, query: String) -> Result<Vec<Value>> {
        super::store::read(&self.state, &query).await
    }
    pub async fn write(&self, query: String) -> Result<Value> {
        super::store::write(&self.state, &query).await
    }
    pub fn customer_match(&self, id: &str) -> Result<String> {
        Ok(format!(
            r#"{} $c isa bill_customer,has bill_key "{}";(merchant:$t,customer:$c) isa bill_account;"#,
            self.prefix(),
            schema::id(id)?
        ))
    }
    pub async fn customer(&self, id: &str) -> Result<Value> {
        let rows = self
            .read(format!(
                "{} fetch {{{CUSTOMER_DATA}}};",
                self.customer_match(id)?
            ))
            .await?;
        ensure!(
            rows.len() == 1,
            "Customer unavailable for configured merchant"
        );
        Ok(rows[0].clone())
    }
    pub async fn ensure_customer(&self, customer: &Value, plugin_id: &str) -> Result<String> {
        let plugin = self.runtime.financial(plugin_id)?;
        plugin.ready()?;
        let current = Self::binding(customer, plugin_id)?;
        if !current.is_empty() {
            ensure!(
                current != "pending",
                "Customer creation outcome needs reconciliation"
            );
            opaque_id(&current)?;
            return Ok(current);
        }
        let id = string(customer, "customer_id")?;
        let before = string(customer, "bindings")?;
        let mut bindings: std::collections::HashMap<String, String> = serde_json::from_str(before)?;
        let expected = if !plugin.capabilities().idempotent_customers {
            bindings.insert(plugin_id.into(), "pending".into());
            let pending = serde_json::to_string(&bindings)?;
            self.write(format!(r#"{} $c has bill_bindings {};select $t,$c;distinct;update $c has bill_bindings {};fetch {{"id":$c.bill_key}};"#,self.customer_match(id)?,schema::quoted(before),schema::quoted(&pending))).await?;
            pending
        } else {
            before.into()
        };
        let external = plugin
            .create_customer(id, string(customer, "name")?, string(customer, "email")?)
            .await?;
        opaque_id(&external)?;
        bindings.insert(plugin_id.into(), external.clone());
        let next = serde_json::to_string(&bindings)?;
        self.write(format!(r#"{} $c has bill_bindings {};select $t,$c;distinct;update $c has bill_bindings {};fetch {{"id":$c.bill_key}};"#,self.customer_match(id)?,schema::quoted(&expected),schema::quoted(&next))).await?;
        Ok(external)
    }
    /// At most ten due occurrences per invocation; each sequence advances atomically
    /// with its invoice. A conflict aborts before any provider request.
    pub async fn enqueue_due(&self, now: i64) -> Result<usize> {
        let rows=self.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;
   $p has bill_status "active",has bill_next $next;$next <= {now};
   let $following,$status,$unit,$every=bill_next_occurrence($p);select $c,$p,$next,$following,$status,$unit,$every;distinct;sort $next;limit 10;
   fetch {{"plan_id":$p.bill_key,"anchor":$p.bill_anchor,"sequence":$p.bill_sequence,"following":$following,"status":$status,"unit":$unit,"every":$every,"next":$next}};"#,self.prefix())).await?;
        let mut count = 0;
        for row in rows {
            let sequence = number(&row, "sequence")?;
            let following = number(&row, "following")?;
            let status = string(&row, "status")?;
            let every = u32::try_from(number(&row, "every")?)?;
            let period = match string(&row, "unit")? {
                "once" => dog_queue::calendar::CalendarPeriod::Once,
                "days" => dog_queue::calendar::CalendarPeriod::Days { every },
                "months" => dog_queue::calendar::CalendarPeriod::Months { every },
                _ => anyhow::bail!("Unsupported native calendar unit"),
            };
            let anchor = number(&row, "anchor")?;
            let unit = schema::quoted(string(&row, "unit")?);
            let next = period
                .occurrence(anchor, u32::try_from(following)?)?
                .unwrap_or(0);
            let plan = schema::id(string(&row, "plan_id")?)?;
            let invoice = uuid::Uuid::new_v4();
            self.write(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;
    $p has bill_key "{plan}",has bill_status "active",has bill_sequence {sequence},has bill_anchor {anchor},has bill_next $issued,has bill_due_days $days,has bill_amount $amount,has name $name;
    $issued <= {now};let $due=$issued+($days*86400);let $following,$next_status,$unit,$every=bill_next_occurrence($p);$following=={following};$next_status=="{status}";$unit=={unit};$every=={every};
    select $t,$p,$name,$amount,$issued,$due,$following;distinct;
    update $p has bill_sequence == $following,has bill_next {next},has bill_status "{status}";
    insert $i isa bill_invoice,has bill_key "{invoice}",has name == $name,has bill_amount == $amount,has bill_anchor == $issued,has bill_next == $due,has bill_sequence {sequence},has bill_choice "manual",has bill_provider "",has bill_status "queued",has bill_external_id "",has bill_url "",has bill_updated {now},has bill_revision "initial";
    (plan:$p,invoice:$i) isa bill_invoice_owner;fetch {{"id":$i.bill_key}};"#,self.prefix())).await?;
            count += 1;
        }
        Ok(count)
    }
    pub fn invoice_match(&self, id: &str) -> Result<String> {
        Ok(format!(
            r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;$i has bill_key "{}";"#,
            self.prefix(),
            schema::id(id)?
        ))
    }
    pub async fn issue(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        // Read customer settings at claim time. Revision validation below rejects a
        // preference or consent change racing the claim.
        let rows=self.read(format!(r#"{} fetch {{{CUSTOMER_DATA},"invoice_id":$i.bill_key,"description":$i.name,"amount":$i.bill_amount,"issued":$i.bill_anchor,"due":$i.bill_next,"mode":bill_plan_payment_choice($c,$p)}};"#,self.invoice_match(id)?)).await?;
        ensure!(rows.len() == 1, "Invoice missing");
        let row = &rows[0];
        // The native model is the sole authority; Rust only decodes its decision.
        let mode: Mode = serde_json::from_value(row["mode"].clone())?;
        let plugin_id = if mode == Mode::Automatic {
            self.method_plugin(row)?
        } else {
            string(row, "manual_provider")?.to_owned()
        };
        let plugin = self.runtime.financial(&plugin_id)?;
        plugin.ready()?;
        ensure!(
            mode != Mode::Automatic || plugin.capabilities().automatic_collection,
            "Selected plugin cannot collect automatically"
        );
        self.write(format!(r#"{} let $allowed=bill_can_issue($c,$p,$i,{},"{}");$allowed==true;select $t,$i;distinct;update $i has bill_status "issuing",has bill_provider {},has bill_choice "{}",has bill_updated {now},has bill_revision {};fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?,schema::quoted(string(row,"revision")?),schema::word(mode),schema::quoted(&plugin_id),schema::word(mode),schema::quoted(string(row,"revision")?))).await?;
        // Once claimed, an ambiguous failure is attention-required, never a fresh charge retry.
        let result=async{
   let customer=self.ensure_customer(row,&plugin_id).await?;
   let request=InvoiceRequest{id:id.into(),customer,automatic:mode==Mode::Automatic,amount_minor:number(row,"amount")?,currency:"USD".into(),description:string(row,"description")?.into(),issued:number(row,"issued")?,due:number(row,"due")?};
   let external=plugin.create_invoice(&request).await?;opaque_id(&external)?;
   self.write(format!(r#"{} $i has bill_status "issuing",has bill_external_id "";select $t,$i;distinct;update $i has bill_external_id {};fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?,schema::quoted(&external))).await?;
   let observation=plugin.finalize_invoice(&request,&external).await?;
   self.apply_observation(id,&plugin_id,&observation).await
  }.await;
        if result.is_err() {
            let _=self.write(format!(r#"{} $i has bill_status "issuing";select $t,$i;distinct;update $i has bill_status "attention";fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?)).await;
        }
        result
    }
    pub async fn apply_observation(
        &self,
        id: &str,
        plugin_id: &str,
        value: &InvoiceObservation,
    ) -> Result<()> {
        let external = &value.external_id;
        opaque_id(external)?;
        let status = value.state.as_str();
        let url = &value.payment_url;
        let now = chrono::Utc::now().timestamp();
        let record = self.validate_observation(id, plugin_id, value).await?;
        let row = &record;
        // Older deliveries may never undo a paid/void observation. Refunds and disputes
        // are distinct events, not a transition back to unpaid.
        if row["transition"] != true {
            return Ok(());
        }
        self.write(format!(r#"{} $i has bill_status {};$c has bill_bindings {};let $allowed=bill_observation_transition($i,"{status}");$allowed==true;select $t,$i;distinct;update $i has bill_status "{status}",has bill_external_id {},has bill_url {},has bill_updated {now};fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?,schema::quoted(string(row,"status")?),schema::quoted(string(row,"bindings")?),schema::quoted(external),schema::quoted(url))).await?;
        Ok(())
    }
    pub async fn validate_observation(
        &self,
        id: &str,
        plugin_id: &str,
        value: &InvoiceObservation,
    ) -> Result<Value> {
        opaque_id(&value.external_id)?;
        opaque_id(&value.customer)?;
        if !value.payment_url.is_empty() {
            let url = reqwest::Url::parse(&value.payment_url)?;
            ensure!(
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none(),
                "Unsafe payment destination"
            );
        }
        let rows=self.read(format!(r#"{} let $valid=bill_observation_matches($i,{},{},{},{},{});$valid==true;let $transition=bill_observation_transition($i,{});fetch {{"bindings":$c.bill_bindings,"status":$i.bill_status,"transition":$transition}};"#,self.invoice_match(id)?,schema::quoted(&value.invoice_id),schema::quoted(plugin_id),schema::quoted(&value.external_id),value.amount_minor,schema::quoted(&value.currency),schema::quoted(value.state.as_str()))).await?;
        ensure!(rows.len() == 1, "Invoice missing");
        let row = &rows[0];
        // Customer reference decoding is a provider-binding attestation. Domain
        // invoice binding, money, currency and terminal-state decisions are native.
        ensure!(
            Self::binding(row, plugin_id)? == value.customer,
            "Invoice customer binding mismatch"
        );
        Ok(row.clone())
    }
    pub async fn reconcile(&self, id: &str) -> Result<()> {
        let rows = self
            .read(format!(
                r#"{} fetch {{"provider":$i.bill_provider,"external":$i.bill_external_id}};"#,
                self.invoice_match(id)?
            ))
            .await?;
        ensure!(rows.len() == 1, "Invoice missing");
        let row = &rows[0];
        let plugin_id = string(row, "provider")?;
        let external = string(row, "external")?;
        ensure!(!external.is_empty(),"Unknown provider outcome: operator must locate the existing invoice; never recreate it");
        let observation = self
            .runtime
            .financial(plugin_id)?
            .observe_invoice(external)
            .await?;
        self.apply_observation(id, plugin_id, &observation).await
    }
    /// One off-session attempt per invoice. A failed or uncertain attempt is
    /// reconciled and the customer receives a payment link; it is never recharged.
    pub async fn collect_due(&self, id: &str, now: i64) -> Result<()> {
        let rows=self.read(format!(r#"{} let $allowed=bill_can_collect($c,$p,$i,{now});$allowed==true;fetch {{{CUSTOMER_DATA},"external":$i.bill_external_id,"plugin":$i.bill_provider}};"#,self.invoice_match(id)?)).await?;
        if rows.is_empty() {
            return Ok(());
        }
        ensure!(rows.len() == 1, "Ambiguous invoice");
        let row = &rows[0];
        let selected = self.runtime.financial(string(row, "plugin")?)?;
        ensure!(
            selected.capabilities().automatic_collection,
            "Automatic collection unsupported"
        );
        selected.ready()?;
        let consent = string(row, "revision")?;
        let attempted = format!("attempted:{consent}");
        self.write(format!(r#"{} $i has bill_revision {};$c has bill_revision {};let $approved=bill_can_collect($c,$p,$i,{now});$approved==true;select $t,$i;distinct;update $i has bill_revision {},has bill_updated {now};fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?,schema::quoted(consent),schema::quoted(consent),schema::quoted(&attempted))).await?;
        let _result = selected
            .collect(string(row, "external")?, id, string(row, "payment_method")?)
            .await;
        // Declines and uncertain responses may still have created a payment.
        // Observe fresh state before notifying; never repeat the claimed attempt.
        self.reconcile(id).await
    }
    // Rotate attempted work even when a provider errors or the tick is cancelled.
    // Otherwise a permanently failing first batch can starve every later invoice.
    pub async fn mark_attempt(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.write(format!(r#"{} select $t,$i;distinct;update $i has bill_updated {now};fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?)).await?;
        Ok(())
    }
    pub async fn tick(&self) -> Result<Value> {
        let scheduled = self.enqueue_due(chrono::Utc::now().timestamp()).await?;
        let rows=self.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;not {{(invoice:$i,notice:$archive) isa bill_notice_owner;$archive has bill_status "imported-history";}};$i has bill_status "queued",has bill_updated $updated,has bill_key $id;select $i,$updated,$id;distinct;sort $updated,$id;limit 10;fetch {{"id":$i.bill_key}};"#,self.prefix())).await?;
        let mut issued = 0;
        let mut errors = 0;
        for row in rows {
            let id = string(&row, "id")?;
            self.mark_attempt(id).await?;
            if self.issue(id).await.is_ok() {
                issued += 1
            } else {
                errors += 1
            }
        }
        let rows=self.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;not {{(invoice:$i,notice:$archive) isa bill_notice_owner;$archive has bill_status "imported-history";}};$i has bill_external_id $external;$external!="";$i has bill_status $status;$status!="paid";$status!="void";$i has bill_updated $updated;select $i,$updated;distinct;sort $updated;limit 20;fetch {{"id":$i.bill_key}};"#,self.prefix())).await?;
        let mut reconciled = 0;
        for row in rows {
            let id = string(&row, "id")?;
            self.mark_attempt(id).await?;
            if self.reconcile(id).await.is_ok() {
                reconciled += 1;
                if self
                    .collect_due(id, chrono::Utc::now().timestamp())
                    .await
                    .is_err()
                {
                    errors += 1
                }
            } else {
                errors += 1
            }
        }
        let (notices, notification_errors) = self.send_notices().await?;
        errors += notification_errors;
        Ok(
            json!({"scheduled":scheduled,"issued":issued,"reconciled":reconciled,"notices":notices,"errors":errors}),
        )
    }
}
pub fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing field {key}"))
}
pub fn number(value: &Value, key: &str) -> Result<i64> {
    let number = &value[key];
    if let Some(integer) = number.as_i64() {
        return Ok(integer);
    }
    // TypeDB's JSON transport emits integral values as JSON doubles. Accept only
    // finite, exactly representable integers; never silently round money or dates.
    let number = number
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("Missing integer {key}"))?;
    ensure!(
        number.is_finite() && number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0,
        "Inexact integer {key}"
    );
    Ok(number as i64)
}
