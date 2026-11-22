//! Presentation templates are data; authorization and delivery decisions stay in TypeDB.
use crate::billing::{
    billing_schema::quoted,
    engine::{string, Engine},
};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;

pub fn render(template: &str, values: &BTreeMap<&str, String>) -> Result<String> {
    let tokens = regex::Regex::new(r"\{([^{}]+)\}")?;
    let mut result = String::new();
    let mut end = 0;
    for token in tokens.captures_iter(template) {
        let matched = token.get(0).context("Template token")?;
        let literal = &template[end..matched.start()];
        ensure!(!literal.contains(['{', '}']), "Malformed template");
        result.push_str(literal);
        result.push_str(values.get(&token[1]).context("Unknown template variable")?);
        end = matched.end();
    }
    let remainder = &template[end..];
    ensure!(!remainder.contains(['{', '}']), "Malformed template");
    result.push_str(remainder);
    Ok(result)
}
impl Engine {
    pub(crate) async fn checkout_message(
        &self,
        invoice: &str,
        kind: &str,
        mut values: BTreeMap<&str, String>,
        fallback: (String, String),
    ) -> Result<(String, String)> {
        let rows=self.read(format!(r#"{} let $configuration in cfg_invoice_profile($i);(configuration:$configuration,message:$message) isa commerce_message_owner;$message has cfg_message_kind {};fetch {{"subject":$message.cfg_message_subject,"text":$message.cfg_message_text,"merchant":$configuration.cfg_brand,"support":$configuration.cfg_support,"timezone":$configuration.cfg_timezone,"due":$i.bill_next}};"#,self.invoice_match(invoice)?,quoted(kind))).await?;
        ensure!(rows.len() <= 1, "Ambiguous checkout template");
        let Some(row) = rows.first() else {
            return Ok(fallback);
        };
        values.insert("merchant", string(row, "merchant")?.into());
        values.insert("support", string(row, "support")?.into());
        if kind != "refund" {
            let due = crate::billing::engine::number(row, "due")?;
            let date = jiff::Timestamp::from_second(due)?;
            // A raw data client can publish a syntactically valid zone unknown to
            // this tzdb. UTC preserves delivery and an unambiguous timestamp.
            let zone = jiff::tz::TimeZone::get(string(row, "timezone")?)
                .unwrap_or(jiff::tz::TimeZone::UTC);
            values.insert(
                "due_at",
                date.to_zoned_datetime(zone)
                    .strftime("%Y-%m-%d %H:%M %:z %Q")
                    .to_string(),
            );
        }
        let subject = render(string(row, "subject")?, &values)?;
        ensure!(
            !subject.contains(['\r', '\n']),
            "Invalid notification subject"
        );
        let text = render(string(row, "text")?, &values)?;
        // Tenant copy cannot remove the independently verified amount and record identity.
        Ok((
            subject,
            format!(
                "{text}\n\nVerified payment record\nInvoice: {}\nAmount: {}\nRefund: {}\n{}",
                values.get("invoice_id").context("invoice")?,
                values.get("amount").context("amount")?,
                values.get("refund_id").context("refund")?,
                fallback.1
            ),
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_are_plain_text_and_never_recursively_expanded() {
        let values = BTreeMap::from([
            ("merchant", "<b>{amount}</b>".into()),
            ("amount", "USD 1.00".into()),
        ]);
        assert_eq!(
            render("Hello {merchant}: {amount}", &values).unwrap(),
            "Hello <b>{amount}</b>: USD 1.00"
        );
        assert!(render("{unknown}", &values).is_err());
        assert!(render("{{amount}", &values).is_err());
    }
}
