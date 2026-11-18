//! One durable provider submission per request, followed only by read-only recovery.
use crate::billing::{
    billing_schema::quoted as q,
    engine::{number, string, Engine},
};
use crate::finance::{Message, RefundObservation, RefundState};
use anyhow::{ensure, Result};
impl Engine {
    fn refund_match(&self, id: &str) -> String {
        format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;(invoice:$i,request:$r) isa commerce_refund_owner;$r has cx_id {};",self.prefix(),q(id))
    }
    pub async fn process_refunds(&self) -> Result<usize> {
        if !self.runtime.sandbox {
            return Ok(0);
        }
        let rows=self.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;(invoice:$i,request:$r) isa commerce_refund_owner;$r has cx_refund_status $status,has cx_created $created;$status!='succeeded';$status!='failed';select $r,$i,$c,$created;sort $created;limit 10;fetch {{'id':$r.cx_id,'status':$r.cx_refund_status,'cancel':cx_refund_cancelled($r),'amount':$r.cx_amount,'plugin':$i.bill_provider,'invoice':$i.bill_external_id,'customer':$c.bill_key}};",self.prefix())).await?;
        let mut count = 0;
        for row in rows {
            let id = string(&row, "id")?;
            if row["cancel"] == true {
                self.write(format!("{} let $cancel=cx_refund_cancelled($r);$cancel==true;select $t,$r;update $r has cx_refund_status 'failed';match $r has cx_busy $busy;delete has $busy of $r;fetch {{'id':$r.cx_id}};",self.refund_match(id))).await?;
                continue;
            }
            let plugin_id = string(&row, "plugin")?;
            let plugin = self.runtime.financial(plugin_id)?;
            if !plugin.capabilities().refunds {
                continue;
            }
            plugin.ready()?;
            let invoice = string(&row, "invoice")?;
            let observation = if row["status"] == "queued" {
                let at = crate::clock::native_utc();
                let claim=self.write(format!("{} let $allowed=cx_refund_claim($t,$r,{at});$allowed==true;select $t,$r;distinct;update $r has cx_refund_status 'processing';fetch {{'id':$r.cx_id}};",self.refund_match(id))).await;
                if let Err(e) = claim {
                    // A raced/uncertain claim never authorizes a provider submission.
                    if e.downcast_ref::<dog_core::DogError>()
                        .is_some_and(|e| e.kind == dog_core::ErrorKind::Conflict)
                    {
                        continue;
                    }
                    return Err(e);
                }
                match plugin.refund(invoice, id, number(&row, "amount")?).await {
                    Ok(value) => Some(value),
                    Err(error)
                        if error
                            .downcast_ref::<crate::finance::RefundNotSubmitted>()
                            .is_some() =>
                    {
                        self.write(format!("{} let $valid=cx_refund_not_submitted($r);$valid==true;select $t,$r;update $r has cx_refund_status 'failed';match $r has cx_busy $busy;delete has $busy of $r;fetch {{'id':$r.cx_id}};",self.refund_match(id))).await?;
                        None
                    }
                    Err(_) => {
                        self.write(format!("{} $r has cx_refund_status 'processing';select $t,$r;update $r has cx_refund_status 'attention';fetch {{'id':$r.cx_id}};",self.refund_match(id))).await?;
                        None
                    }
                }
            } else {
                plugin.find_refund(invoice, id).await?
            };
            if let Some(value) = observation {
                // Customer/external reference attestation is the plugin boundary; money,
                // request binding and state transition are checked natively in this write.
                let customer = self.customer(string(&row, "customer")?).await?;
                ensure!(
                    Self::binding(&customer, plugin_id)? == value.customer,
                    "Refund customer binding mismatch"
                );
                self.apply_refund(id, plugin_id, &value).await?;
                count += 1;
            }
        }
        self.send_refund_notices().await?;
        Ok(count)
    }
    async fn apply_refund(&self, id: &str, plugin: &str, v: &RefundObservation) -> Result<()> {
        crate::finance::opaque_id(&v.external_id)?;
        let next = v.state.as_str();
        let release = if v.state == RefundState::Pending {
            String::new()
        } else {
            "match $r has cx_busy $busy;delete has $busy of $r;".into()
        };
        let result = self.write(format!("{} let $valid=cx_refund_observation($r,{},{},{},{},{},{},{});$valid==true;select $t,$r;distinct;update $r has cx_refund_status {},has cx_external {},has cx_external_binding {};{release}fetch {{'id':$r.cx_id}};",self.refund_match(id),q(&v.request_id),q(&v.invoice_id),q(plugin),v.amount_minor,q(&v.currency),q(&v.external_id),q(next),q(next),q(&v.external_id),q(&format!("{}:{plugin}:{}",self.merchant,v.external_id)))).await;
        if let Err(error) = result {
            // An identical observation may already be committed by another worker.
            // This read never authorizes a second provider submission.
            let recorded = self.read(format!("{} $r has cx_refund_status {},has cx_external {},has cx_external_binding {};fetch {{'id':$r.cx_id}};",self.refund_match(id),q(next),q(&v.external_id),q(&format!("{}:{plugin}:{}",self.merchant,v.external_id)))).await?;
            if recorded.len() != 1 {
                return Err(error);
            }
        }
        Ok(())
    }
    async fn send_refund_notices(&self) -> Result<()> {
        if self.runtime.notification.is_none() {
            return Ok(());
        }
        let rows=self.read(format!("{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;(invoice:$i,request:$r) isa commerce_refund_owner;$r has cx_refund_status 'succeeded',has cx_id $id;let $key=$id+'-refund-receipt';not {{$n isa bill_notice,has bill_key == $key;}};select $r,$i,$c,$key;limit 10;fetch {{'id':$r.cx_id,'invoice':$i.bill_key,'amount':$r.cx_amount,'email':$c.bill_email}};",self.prefix())).await?;
        for row in rows {
            let key = format!("{}-refund-receipt", string(&row, "id")?);
            let recipient = string(&row, "email")?;
            self.runtime.validate_delivery(recipient)?;
            let at = crate::clock::unix_seconds();
            let invoice = string(&row, "invoice")?;
            self.write(format!("{} not {{$n isa bill_notice,has bill_key {};}};select $t,$i;distinct;insert $n isa bill_notice,has bill_key {},has bill_status 'sending',has bill_updated {at};(invoice:$i,notice:$n) isa bill_notice_owner;fetch {{'id':$n.bill_key}};",self.invoice_match(invoice)?,q(&key),q(&key))).await?;
            let amount = number(&row, "amount")?;
            let result=self.runtime.send(Message{to:recipient.into(),key:key.clone(),subject:"[TEST — no real payment] Refund confirmed".into(),text:format!("Your refund of USD {}.{:02} for invoice {invoice} is confirmed by the payment provider. Request: {}.",amount/100,amount%100,string(&row,"id")?)}).await;
            let status = result
                .map(|id| format!("accepted:{id}"))
                .unwrap_or_else(|_| "attention".into());
            self.write(format!("{} (invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key {},has bill_status 'sending';select $t,$n;update $n has bill_status {};fetch {{'id':$n.bill_key}};",self.invoice_match(invoice)?,q(&key),q(&status))).await?;
        }
        Ok(())
    }
}
