//! Durable notification claims. An uncertain send requires reconciliation, not blind resend.
use super::{
    billing_schema::quoted,
    engine::{number, string, Engine},
};
use anyhow::{ensure, Result};
impl Engine {
    pub async fn notify_invoice(&self, id: &str) -> Result<usize> {
        if self.runtime.notification.is_none() {
            return Ok(0);
        }
        let archived = self.read(format!(r#"{} (invoice:$i,notice:$archive) isa bill_notice_owner;$archive has bill_status "imported-history";fetch {{"id":$i.bill_key}};"#,self.invoice_match(id)?)).await?;
        if !archived.is_empty() {
            return Ok(0);
        }
        // Fresh provider state avoids reminders based only on an old local observation.
        self.reconcile(id).await?;
        let now = crate::clock::unix_seconds();
        let interval = self.runtime.reminder_days;
        let rows=self.read(format!(r#"{} let $kind,$bucket=bill_notice_decision($i,{now},{interval});fetch {{"email":$c.bill_email,"description":$i.name,"amount":$i.bill_amount,"due":$i.bill_next,"status":$i.bill_status,"url":$i.bill_url,"kind":$kind,"bucket":$bucket}};"#,self.invoice_match(id)?)).await?;
        let Some(row) = rows.first() else {
            return Ok(0);
        };
        ensure!(rows.len() == 1, "Ambiguous invoice notice");
        let due = number(row, "due")?;
        let category = string(row, "kind")?;
        let bucket = number(row, "bucket")?;
        let kind = if category == "reminder" {
            format!("reminder-{bucket}")
        } else {
            category.into()
        };
        let key = format!("{id}-{kind}");
        let exists=self.read(format!(r#"{} (invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key {};fetch {{"status":$n.bill_status}};"#,self.invoice_match(id)?,quoted(&key))).await?;
        if !exists.is_empty() {
            return Ok(0);
        }
        let recipient = string(row, "email")?;
        self.runtime.validate_delivery(recipient)?;
        let amount = number(row, "amount")?;
        let description = string(row, "description")?;
        let url = string(row, "url")?;
        let (subject, body) = if kind == "receipt" {
            ("Payment received",format!("Thank you. Your invoice {id} for {description} is marked paid.\nAmount: USD {}.{:02}\nView your invoice: {url}\n",amount/100,amount%100))
        } else {
            (if now>=due {"Payment reminder"} else {"Your invoice"},format!("Your invoice {id} for {description} is {}.\nAmount: USD {}.{:02}\nDue: {} UTC\nView and pay securely: {url}\nIf you need more time, please contact us. If payment is already processing, no further action is needed.\n",if now>=due{"due"}else{"ready"},amount/100,amount%100,chrono::DateTime::from_timestamp(due,0).ok_or_else(||anyhow::anyhow!("Invalid date"))?.format("%Y-%m-%d")))
        };
        let subject = if !self.runtime.sandbox {
            subject.to_owned()
        } else {
            format!("[TEST — no real payment] {subject}")
        };
        self.write(format!(r#"{} let $kind,$bucket=bill_notice_decision($i,{now},{interval});$kind=={};$bucket=={bucket};not {{$n isa bill_notice,has bill_key {};}};select $t,$i;distinct;
   insert $n isa bill_notice,has bill_key {},has bill_status "sending",has bill_updated {now};(invoice:$i,notice:$n) isa bill_notice_owner;fetch {{"id":$n.bill_key}};"#,self.invoice_match(id)?,quoted(category),quoted(&key),quoted(&key))).await?;
        let result = self
            .runtime
            .send(crate::finance::Message {
                to: recipient.into(),
                subject,
                text: body,
                key: key.clone(),
            })
            .await;
        let status = match &result {
            Ok(id) => format!("accepted:{id}"),
            Err(_) => "attention".into(),
        };
        self.write(format!(r#"{} (invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key {},has bill_status "sending";select $t,$n;distinct;update $n has bill_status {};fetch {{"id":$n.bill_key}};"#,self.invoice_match(id)?,quoted(&key),quoted(&status))).await?;
        result?;
        Ok(1)
    }
    pub async fn send_notices(&self) -> Result<(usize, usize)> {
        if self.runtime.notification.is_none() {
            return Ok((0, 0));
        }
        let rows = self.notification_candidates().await?;
        let mut sent = 0;
        let mut errors = 0;
        for row in rows {
            let id = string(&row, "id")?;
            self.mark_attempt(id).await?;
            match self.notify_invoice(id).await {
                Ok(count) => sent += count,
                Err(_) => errors += 1,
            }
        }
        Ok((sent, errors))
    }
    pub async fn notification_candidates(&self) -> Result<Vec<serde_json::Value>> {
        let rows=self.read(format!(r#"{} (merchant:$t,customer:$c) isa bill_account;(customer:$c,plan:$p) isa bill_plan_owner;(plan:$p,invoice:$i) isa bill_invoice_owner;not {{(invoice:$i,notice:$archive) isa bill_notice_owner;$archive has bill_status "imported-history";}};
  {{$i has bill_status "open";}} or {{$i has bill_status "paid";not {{(invoice:$i,notice:$n) isa bill_notice_owner;$n has bill_key $key;$i has bill_key $id;let $receipt=$id+"-receipt";$key==$receipt;}};}};
  $i has bill_updated $updated;select $i,$updated;distinct;sort $updated;limit 20;fetch {{"id":$i.bill_key}};"#,self.prefix())).await?;
        Ok(rows)
    }
}
