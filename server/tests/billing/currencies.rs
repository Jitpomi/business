//! Real Stripe TEST presentment/refund contracts. Stable keys and a preserved journal;
//! a repeated test cannot submit another payment under an uncertain acknowledgement.
use super::*;
use server::finance::{FinancialPlugin, InvoiceRequest, InvoiceState, RefundState};
use server::plugins::{builtin_transport::Providers, BuiltinFinancial};
#[tokio::test]
#[ignore = "Explicit Stripe TEST key and new private journal directory; synthetic EUR and JPY payments/refunds only"]
async fn stripe_currency_payment_and_refund_contracts() -> Result<()> {
    server::tls::initialize();
    let secrets: HashMap<String, String> =
        dotenvy::from_path_iter(std::env::var("BILLING_TEST_ENV")?)?
            .collect::<std::result::Result<_, _>>()?;
    let token = secrets
        .get("STRIPE_SECRET_KEY")
        .context("TEST key required")?;
    ensure!(
        token.starts_with("sk_test_"),
        "Only TEST credentials allowed"
    );
    let root = std::path::PathBuf::from(std::env::var("CURRENCY_TEST_ROOT")?);
    ensure!(root.is_absolute()&&!root.exists(),"New private journal directory required; inspect existing attempts instead of replaying them");
    std::fs::create_dir(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    let plugin = BuiltinFinancial {
        transport: Providers::connected(false, Some(token.clone()), None, None, None)?,
        provider: Provider::Stripe,
        webhook_secret: None,
    };
    for currency in ["EUR", "JPY"] {
        let id = uuid::Uuid::new_v4().to_string();
        let refund = uuid::Uuid::new_v4().to_string();
        let path = root.join(format!("{currency}.json"));
        let mut journal =
            json!({"currency":currency,"id":id,"refund_request":refund,"phase":"customer"});
        let save = |v: &serde_json::Value| -> Result<()> {
            std::fs::write(&path, serde_json::to_vec_pretty(v)?)?;
            Ok(())
        };
        save(&journal)?;
        let customer = plugin
            .create_customer(
                &id,
                "TEST currency checkout — no real money",
                "dev@jitpomi.com",
            )
            .await?;
        journal["customer"] = json!(customer);
        journal["phase"] = json!("invoice");
        save(&journal)?;
        let now = server::clock::unix_seconds();
        let request = InvoiceRequest {
            id: id.clone(),
            customer: customer.clone(),
            automatic: false,
            amount_minor: 1000,
            currency: currency.into(),
            description: "TEST currency checkout — no real money".into(),
            issued: now,
            due: now + 86400,
        };
        let external = plugin.create_invoice(&request).await?;
        journal["invoice"] = json!(external);
        save(&journal)?;
        let observed = plugin.finalize_invoice(&request, &external).await?;
        ensure!(
            observed.currency == currency
                && observed.amount_minor == 1000
                && observed.state == InvoiceState::Open
        );
        journal["phase"] = json!("manual_test_payment_attempt");
        save(&journal)?;
        let method = plugin
            .transport
            .stripe(
                reqwest::Method::POST,
                "/payment_methods/pm_card_visa/attach",
                &format!("currency-method-{id}"),
                &pairs(&[("customer", &customer)]),
            )
            .await?;
        let method = method["id"].as_str().context("Test method id")?;
        let paid = plugin
            .transport
            .stripe(
                reqwest::Method::POST,
                &format!("/invoices/{external}/pay"),
                &format!("currency-pay-{id}"),
                &pairs(&[("payment_method", method)]),
            )
            .await?;
        ensure!(paid["status"] == "paid" && paid["amount_paid"] == 1000);
        let settled = plugin.observe_invoice(&external).await?;
        ensure!(
            settled.state == InvoiceState::Paid
                && settled.currency == currency
                && settled.amount_minor == 1000
        );
        let history = plugin.history(&external).await?;
        ensure!(
            history["currency"] == currency
                && history["payments"].as_array().is_some_and(|p| p.len() == 1)
        );
        journal["phase"] = json!("refund_attempt");
        save(&journal)?;
        let returned = plugin.refund(&external, &refund, 200).await?;
        ensure!(
            returned.currency == currency
                && returned.amount_minor == 200
                && returned.state == RefundState::Succeeded
        );
        let recovered = plugin
            .find_refund(&external, &refund)
            .await?
            .context("Read-only refund recovery")?;
        ensure!(
            recovered.external_id == returned.external_id
                && recovered.currency == currency
                && recovered.amount_minor == 200
        );
        let history = plugin.history(&external).await?;
        let refunds = history["payments"][0]["refunds"]
            .as_array()
            .context("Refund history")?;
        ensure!(refunds.len() == 1 && refunds[0]["amount_cents"] == 200);
        journal["phase"] = json!("verified");
        journal["refund_external"] = json!(returned.external_id);
        save(&journal)?;
        println!("{currency}: exact provider units, settled TEST payment, one partial refund, read-only recovery and currency-bound history verified");
    }
    Ok(())
}
