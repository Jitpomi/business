use super::*;
#[test]
fn money_is_exact_and_payment_links_are_trusted() {
    assert_eq!(decimal_cents(&json!("12.50")).unwrap(), 1250);
    for value in [
        json!("0.001"),
        json!("-1"),
        json!("1e3"),
        json!("9223372036854775807"),
    ] {
        assert!(decimal_cents(&value).is_err());
    }
    assert!(trusted_url(
        "https://invoice.stripe.com/i/test_fixture",
        "invoice.stripe.com"
    )
    .is_ok());
    for url in [
        "http://invoice.stripe.com/i/x",
        "https://invoice.stripe.com.evil.example/i/x",
        "https://user@invoice.stripe.com/i/x",
        "https://invoice.stripe.com:444/i/x",
    ] {
        assert!(trusted_url(url, "invoice.stripe.com").is_err());
    }
}
#[test]
fn pending_bank_payment_and_unknown_states_never_look_payable() {
    assert_eq!(
        status(
            Provider::Stripe,
            &json!({"status":"open","business_payment_processing":true})
        )
        .unwrap(),
        "processing"
    );
    assert!(status(Provider::Stripe, &json!({"status":"unexpected"})).is_err());
    assert!(safe_id("in_a/../in_b").is_err());
}
#[test]
fn mercury_invoices_do_not_require_a_stripe_account() {
    let transport = Providers::new(
        "fixture",
        vec![],
        vec![],
        false,
        Some("11111111-1111-4111-8111-111111111111".into()),
    )
    .unwrap();
    let input = InvoiceRequest {
        id: "invoice".into(),
        customer: "customer".into(),
        provider: Provider::Mercury,
        mode: Mode::Manual,
        amount: 123,
        description: "Work".into(),
        issued: 1800000000,
        due: 1800000000,
    };
    let bank_transfer = transport.mercury_invoice_body(&input).unwrap();
    assert_eq!(bank_transfer["creditCardEnabled"], false);
    assert_eq!(bank_transfer["achDebitEnabled"], false);
    assert_eq!(bank_transfer["sendEmailOption"], "DontSend");
    assert_eq!(bank_transfer["lineItems"][0]["unitPrice"], json!(1.23));
    let enabled = transport
        .with_mercury_payment_options(true, true)
        .mercury_invoice_body(&input)
        .unwrap();
    assert_eq!(enabled["creditCardEnabled"], true);
    assert_eq!(enabled["achDebitEnabled"], true);
}
