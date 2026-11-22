//! Executes native commerce policies through the same scoped data adapter as /data.
use anyhow::{ensure, Context, Result};
use dog_typedb::isolation::{AuthenticatedScope, SchemaPolicyAdapter};
use serde_json::{json, Value};
use server::{database, typedb::TypeDBState};
use std::sync::Arc;
use typedb_driver::TransactionType;

async fn write(
    adapter: &SchemaPolicyAdapter,
    scope: &AuthenticatedScope,
    query: String,
) -> Result<Value> {
    let started = std::time::Instant::now();
    eprintln!("Commerce write started ({} bytes)", query.len());
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        adapter.write(scope, json!({"query":query})),
    )
    .await
    .expect("Commerce timeout is a test failure, not an authorization denial")?;
    eprintln!("Commerce write finished in {:?}", started.elapsed());
    ensure!(
        result["ok"]["answers"]
            .as_array()
            .is_some_and(|rows| rows.len() == 1),
        "Write matched no accepted record"
    );
    Ok(result)
}
fn offer(id: &str, kind: &str, unit: i64, min: i64, max: i64, interval: &str) -> String {
    format!(
        r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();
insert $o isa commerce_offer,has cx_id "{id}",has name "Test offer",has cx_created == $now,
has cx_kind "{kind}",has cx_currency "USD",has cx_unit {unit},has cx_min {min},has cx_max {max},has cx_interval "{interval}",has cx_enabled true;
(merchant:$t,offer:$o) isa commerce_catalogue;fetch {{"id":$o.cx_id}};"#
    )
}
fn order(id: &str, offer: &str, unit: i64, quantity: i64, terms: (&str, &str, i64, i64)) -> String {
    let (mode, interval, count, deposit) = terms;
    format!(
        r#"match $c isa bill_customer,has bill_key "customer";$p isa commerce_pricing,has cx_id "standard";
$f isa commerce_offer,has cx_id "{offer}";let $now=request_now();
insert $o isa commerce_order,has cx_id "{id}",has cx_created == $now,has cx_currency "USD",has cx_lines 1,
has cx_mode "{mode}",has cx_interval "{interval}",has cx_count {count},has cx_deposit {deposit};
(customer:$c,pricing:$p,order:$o) isa commerce_order_owner;
$l isa commerce_line,links(order:$o,offer:$f),has cx_id "line-{id}",has cx_created == $now,has cx_unit {unit},has cx_quantity {quantity};
fetch {{"id":$o.cx_id}};"#
    )
}
#[tokio::test]
#[ignore = "Real development TypeDB; generated isolated database; no payment or mail calls"]
async fn native_commerce_pricing_isolation_and_immutable_orders() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    super::support::auth()?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(
        crate::test_driver::connect(
            &std::env::var("TYPEDB_ADDR")?,
            &std::env::var("TYPEDB_USERNAME")?,
            &std::env::var("TYPEDB_PASSWORD")?,
            true,
        )
        .await?,
    );
    let db = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Commerce fixture: {db}");
    database::install(&driver, &db)
        .await
        .context("Install commerce schema")?;
    let result=async {
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(r#"insert
$s isa tenant,has tenant_id "seller",has name "Seller",has status "active",has revision "initial";
$b isa tenant,has tenant_id "buyer",has name "Buyer",has status "active",has revision "initial";
$x isa tenant,has tenant_id "outsider",has name "Outsider",has status "active",has revision "initial";
$u isa person,has identity_id "seller-owner",has name "Seller",has status "active";
$v isa person,has identity_id "buyer-owner",has name "Buyer",has status "active";
$w isa person,has identity_id "outsider-owner",has name "Outsider",has status "active";
(tenant:$s,member:$u) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;
(tenant:$b,member:$v) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;
(tenant:$x,member:$w) isa membership,has membership_role "owner",has start_at 2020-01-01T00:00:00;
$c isa bill_customer,has bill_key "customer",has name "Buyer",has bill_email "buyer@example.invalid",has bill_policy "customer",has bill_choice "manual",has bill_provider "",has bill_bindings "{}",has bill_method_plugin "",has bill_payment_method "",has bill_consent "",has bill_revision "initial";
(merchant:$s,buyer:$b,customer:$c) isa bill_account;"#).await?;
        tx.commit().await?;
        let state=Arc::new(TypeDBState{driver:driver.clone(),database:db.clone()});
        let adapter=SchemaPolicyAdapter::new(state);
        let seller=AuthenticatedScope::new("seller","seller-owner")?;
        let buyer=AuthenticatedScope::new("buyer","buyer-owner")?;
        let outsider=AuthenticatedScope::new("outsider","outsider-owner")?;
        let embed=|id:&str,origin:&str|format!("match $m isa tenant,has tenant_id 'seller';let $at=request_now();insert $e isa commerce_embed_origin,links(merchant:$m),has cx_id '{id}',has cx_origin '{origin}',has cx_created == $at,has cx_enabled true;fetch {{'id':$e.cx_id}};");
        write(&adapter,&seller,embed("embed","https://merchant.example")).await.context("Approve embedding origin")?;
        ensure!(write(&adapter,&buyer,embed("buyer-embed","https://attacker.example")).await.is_err(),"Buyer approved a merchant origin");
        ensure!(write(&adapter,&outsider,embed("foreign-embed","https://attacker.example")).await.is_err(),"Foreign tenant approved origin");
        ensure!(write(&adapter,&seller,embed("duplicate-embed","https://merchant.example")).await.is_err(),"Duplicate origin bypasses revocation");
        ensure!(write(&adapter,&seller,embed("unsafe-embed","http://merchant.example/path")).await.is_err(),"Non-origin URL accepted");
        ensure!(write(&adapter,&buyer,"match $e isa commerce_embed_origin,has cx_id 'embed';update $e has cx_enabled false;fetch {'id':$e.cx_id};".into()).await.is_err(),"Buyer revoked merchant origin");
        ensure!(write(&adapter,&seller,"match $e isa commerce_embed_origin,has cx_id 'embed';update $e has cx_origin 'https://attacker.example';fetch {'id':$e.cx_id};".into()).await.is_err(),"Owner rewrote an immutable origin instead of explicitly approving it");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(format!("match $c isa bill_customer,has bill_key 'customer';$m isa tenant,has tenant_id 'seller';insert $p isa bill_plan,has bill_key 'inline-plan',has name 'Inline fixture',has bill_amount 100,has bill_due_days 1,has bill_anchor 1,has bill_next 1,has bill_sequence 1,has bill_interval 'once',has bill_status 'finished';(customer:$c,plan:$p) isa bill_plan_owner;$i isa bill_invoice,has bill_key 'inline-invoice',has name 'Inline fixture',has bill_amount 100,has bill_anchor 1,has bill_next 1,has bill_sequence 0,has bill_choice 'manual',has bill_provider 'test',has bill_status 'open',has bill_external_id 'provider-invoice',has bill_url 'https://payment.example',has bill_updated 1,has bill_revision 'initial';(plan:$p,invoice:$i) isa bill_invoice_owner;$fact isa commerce_plugin,has cx_id 'inline-fact',has cx_plugin 'test',has cx_enabled true,has cx_inline_payment true,has cx_verified {},has cx_supported_currency 'USD';(merchant:$m,plugin:$fact) isa commerce_plugin_owner;",server::clock::native_utc())).await?;tx.commit().await?;
        let amounts=adapter.read(&buyer,json!({"query":"match $i isa bill_invoice,has bill_key 'inline-invoice';fetch {'partial':cx_inline_amount_valid($i,60),'excess':cx_inline_amount_valid($i,101),'zero':cx_inline_amount_valid($i,0)};"})).await?;let amounts=&amounts["ok"]["answers"][0]["data"];ensure!(amounts["partial"]==true&&amounts["excess"]==false&&amounts["zero"]==false,"Native form balance bounds failed");
        let allowed=|origin:&str|json!({"query":format!("match $i isa bill_invoice,has bill_key 'inline-invoice';fetch {{'allowed':request_inline_payment_allowed($i,'{origin}')}};")});
        let result=adapter.read(&buyer,allowed("https://merchant.example")).await?;
        ensure!(result["ok"]["answers"][0]["data"]["allowed"]==true,"Approved buyer cannot present invoice");
        let tx=driver.transaction(&db,TransactionType::Write).await?;tx.query("match $f isa commerce_plugin,has cx_id 'inline-fact';update $f has cx_inline_payment false;").await?;tx.commit().await?;
        let unavailable=adapter.read(&buyer,allowed("")).await?;ensure!(unavailable["ok"]["answers"][0]["data"]["allowed"]==false,"Disabled or non-sandbox inline attestation was ignored");
        let tx=driver.transaction(&db,TransactionType::Write).await?;tx.query("match $f isa commerce_plugin,has cx_id 'inline-fact';update $f has cx_inline_payment true;").await?;tx.commit().await?;

        for (scope,origin) in [(&seller,"https://merchant.example"),(&buyer,"https://attacker.example")] {
            let result=adapter.read(scope,allowed(origin)).await?;ensure!(result["ok"]["answers"][0]["data"]["allowed"]==false,"Unauthorized payment presentation allowed");
        }
        write(&adapter,&seller,"match $e isa commerce_embed_origin,has cx_id 'embed';update $e has cx_enabled false;fetch {'id':$e.cx_id};".into()).await?;
        let result=adapter.read(&buyer,allowed("https://merchant.example")).await?;ensure!(result["ok"]["answers"][0]["data"]["allowed"]==false,"Revoked origin still allowed");
        let result=adapter.read(&buyer,allowed("")).await?;ensure!(result["ok"]["answers"][0]["data"]["allowed"]==true,"Hosted payment incorrectly requires an embedding origin");
        let tx=driver.transaction(&db,TransactionType::Write).await?;tx.query("match $i isa bill_invoice,has bill_key 'inline-invoice';update $i has bill_status 'paid';").await?;tx.commit().await?;
        let result=adapter.read(&buyer,allowed("")).await?;ensure!(result["ok"]["answers"][0]["data"]["allowed"]==false,"Paid invoice can be presented again");
        write(&adapter,&seller,offer("item","product",1001,1001,1001,"once")).await.context("Create product")?;
        write(&adapter,&seller,offer("monthly","service",2500,2500,2500,"month")).await?;
        write(&adapter,&seller,offer("donation","donation",0,100,100000,"once")).await?;
        write(&adapter,&seller,offer("recurring-donation","donation",0,100,100000,"month")).await?;
        ensure!(write(&adapter,&buyer,offer("forged","product",100,100,100,"once")).await.is_err(),"Buyer created a merchant offer");
        write(&adapter,&seller,r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();
insert $p isa commerce_pricing,has cx_id "standard",has cx_currency "USD",has name "No adjustments",has cx_created == $now,has cx_tax_bp 0,has cx_discount_bp 0,has cx_shipping 0,has cx_enabled true,has cx_instalments true,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "week",has cx_plan_period "month";
(merchant:$t,pricing:$p) isa commerce_pricing_owner;fetch {"id":$p.cx_id};"#.into()).await?;
        for (id,f,unit,q,mode,period,count,deposit) in [
            ("purchase","item",1001,3,"once","once",1,0),
            ("plan","item",1001,1,"instalments","month",3,0),
            ("deposit","item",1001,1,"instalments","week",3,300),
            ("subscription","monthly",2500,1,"recurring","month",0,0),
            ("gift","donation",1234,1,"once","once",1,0),
            ("monthly-gift","recurring-donation",500,1,"recurring","month",0,0)
        ] {write(&adapter,&buyer,order(id,f,unit,q,(mode,period,count,deposit))).await.with_context(||format!("Create {id}"))?;}
        for (id,f,unit,q,mode,period,count,deposit) in [
            ("underpaid","item",1,1,"once","once",1,0),
            ("overpaid","item",1002,1,"once","once",1,0),
            ("tiny-donation","donation",99,1,"once","once",1,0),
            ("large-donation","donation",100001,1,"once","once",1,0),
            ("multiplied-gift","donation",500,2,"once","once",1,0),
            ("bad-period","monthly",2500,1,"recurring","year",0,0),
            ("bad-plan","item",1001,1,"instalments","month",360,0),
            ("too-many-parts","item",1001,1,"instalments","month",13,0),
            ("unapproved-cadence","item",1001,1,"instalments","year",3,0),
            ("bad-deposit","item",1001,1,"instalments","month",3,1001),
            ("single-instalment","item",1001,1,"instalments","month",1,0),
            ("unbounded-once","item",1001,1,"once","once",0,0)
        ] {ensure!(write(&adapter,&buyer,order(id,f,unit,q,(mode,period,count,deposit))).await.is_err(),"Accepted invalid order {id}");}
        ensure!(write(&adapter,&outsider,order("foreign","item",1001,1,("once","once",1,0))).await.is_err(),"Foreign tenant placed an order");
        let visible=adapter.read(&outsider,json!({"query":"match $o isa commerce_order;fetch {'id':$o.cx_id};"})).await?;
        ensure!(visible["ok"]["answers"].as_array().context("answers")?.is_empty(),"Order leaked across tenants");
        for query in [
            "match $o isa commerce_order,has cx_id 'purchase';update $o has cx_count 9;fetch {'id':$o.cx_id};",
            "match $f isa commerce_offer,has cx_id 'item';update $f has cx_unit 1;fetch {'id':$f.cx_id};",
            "match $p isa commerce_pricing,has cx_id 'standard';update $p has cx_discount_bp 10000;fetch {'id':$p.cx_id};",
            "match $o isa commerce_order,has cx_id 'purchase';$f isa commerce_offer,has cx_id 'item';let $now=request_now();insert $l isa commerce_line,links(order:$o,offer:$f),has cx_id 'late-line',has cx_created == $now,has cx_unit 1001,has cx_quantity 1;fetch {'id':$l.cx_id};"
        ] {ensure!(write(&adapter,&buyer,query.into()).await.is_err(),"Accepted an immutable order mutation");}
        ensure!(write(&adapter,&buyer,order("purchase","item",1001,3,("once","once",1,0))).await.is_err(),"Duplicate request created another order");
        write(&adapter,&seller,r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();
insert $p isa commerce_pricing,has cx_id "adjusted",has cx_currency "USD",has name "Published adjustment quote",has cx_created == $now,
has cx_tax_bp 500,has cx_discount_bp 1000,has cx_shipping 100,has cx_enabled true,
has cx_instalments false,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "month";
(merchant:$t,pricing:$p) isa commerce_pricing_owner;fetch {"id":$p.cx_id};"#.into()).await?;
        let disallowed=order("no-financing","item",1001,1,("instalments","month",3,0)).replace("has cx_id \"standard\"", "has cx_id \"adjusted\"");
        ensure!(write(&adapter,&buyer,disallowed).await.is_err(),"Buyer enabled instalments against merchant policy");
        let cart=order("cart","item",1001,3,("once","once",1,0)).replace(
            "fetch {\"id\":$o.cx_id};",
            "$second isa commerce_line,links(order:$o,offer:$f),has cx_id 'cart-second',has cx_created == $now,has cx_unit 1001,has cx_quantity 3;fetch {\"id\":$o.cx_id};"
        );
        let cart=cart.replace("has cx_lines 1", "has cx_lines 2");
        write(&adapter,&buyer,cart.replace("has cx_id \"standard\"", "has cx_id \"adjusted\"")).await?;
        let totals=adapter.read(&buyer,json!({"query":include_str!("../../public/queries/data/commerce-orders.tql")})).await?;
        ensure!(totals["ok"]["answers"].as_array().context("Order projection")?.len()==7,"Native order projection did not return accepted orders");
        for (actor,scope) in [("merchant",&seller),("buyer",&buyer)] {
            let read=adapter.read(scope,json!({"query":"match $o isa commerce_order,has cx_id 'plan';let $amount=request_commerce_due($o,2);fetch {'amount':$amount};"})).await?;
            ensure!(read["ok"]["answers"].as_array().context("Due projection")?.len()==1,"{actor} could not review a scheduled amount");
        }
        // Raw reads independently verify native arithmetic; there is no Rust total calculator.
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        for (id,amounts) in [("plan",vec![333,333,335]),("deposit",vec![300,350,351]),("purchase",vec![3003]),("cart",vec![5781])] {
            for (n,expected) in amounts.into_iter().enumerate() {
                let rows=database::documents(&tx,&format!("match $o isa commerce_order,has cx_id '{id}';let $due=cx_due_amount($o,{n});fetch {{'due':$due}};")).await?;
                ensure!(rows.len()==1&&server::billing::engine::number(&rows[0],"due")?==expected,"Incorrect instalment rounding: {id}/{n}");
            }
        }
        let quotes=database::documents(&tx,"match $o isa commerce_order,has cx_id 'cart';let $subtotal,$discount,$tax,$total=cx_totals($o);fetch {'subtotal':$subtotal,'discount':$discount,'tax':$tax,'total':$total};").await?;
        ensure!(quotes.len()==1&&server::billing::engine::number(&quotes[0],"subtotal")?==6006&&server::billing::engine::number(&quotes[0],"discount")?==600&&server::billing::engine::number(&quotes[0],"tax")?==275&&server::billing::engine::number(&quotes[0],"total")?==5781,"Discount, shipping or tax rounding differed");
        let rows=database::documents(&tx,"match $o isa commerce_order,has cx_id 'plan';let $due=cx_due_amount($o,3);fetch {'due':$due};").await?;ensure!(rows.is_empty(),"Finite plan generated an extra charge");
        tx.close().await?;
        // Every currency is native immutable price context, not a UI label or FX conversion.
        for currency in ["EUR","GBP","CAD","AUD","JPY"] {
            let item=format!("item-{currency}");let policy=format!("policy-{currency}");
            write(&adapter,&seller,offer(&item,"product",1001,1001,1001,"once").replace("has cx_currency \"USD\"",&format!("has cx_currency \"{currency}\""))).await?;
            write(&adapter,&seller,format!(r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();insert $p isa commerce_pricing,has cx_id "{policy}",has cx_currency "{currency}",has name "Currency policy",has cx_created == $now,has cx_tax_bp 500,has cx_discount_bp 1000,has cx_shipping 100,has cx_enabled true,has cx_instalments true,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "month";(merchant:$t,pricing:$p) isa commerce_pricing_owner;fetch {{"id":$p.cx_id}};"#)).await?;
            let proposal=order(&format!("quote-{currency}"),&item,1001,1,("instalments","month",3,0)).replace("has cx_id \"standard\"",&format!("has cx_id \"{policy}\"")).replace("has cx_currency \"USD\"",&format!("has cx_currency \"{currency}\""));
            write(&adapter,&buyer,proposal).await?;
            ensure!(write(&adapter,&buyer,order(&format!("wrong-currency-{currency}"),&item,1001,1,("once","once",1,0))).await.is_err(),"Cross-currency offer accepted");
            let wrong_policy=order(&format!("wrong-policy-{currency}"),&item,1001,1,("once","once",1,0)).replace("has cx_currency \"USD\"",&format!("has cx_currency \"{currency}\""));
            ensure!(write(&adapter,&buyer,wrong_policy).await.is_err(),"USD shipping/discount policy applied to foreign currency");
            ensure!(write(&adapter,&buyer,format!("match $o isa commerce_order,has cx_id 'quote-{currency}';update $o has cx_currency 'USD';fetch {{'id':$o.cx_id}};")).await.is_err(),"Order currency mutated");
        }
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(format!("match $t isa tenant,has tenant_id 'seller';insert $fact isa commerce_plugin,has cx_id 'usd-only',has cx_plugin 'usd-only',has cx_enabled true,has cx_verified {},has cx_supported_currency 'USD';(merchant:$t,plugin:$fact) isa commerce_plugin_owner;",server::clock::native_utc())).await?;tx.commit().await?;
        let unsupported=r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();insert $p isa commerce_pricing,has cx_id "unsupported-policy",has cx_currency "JPY",has cx_plugin "usd-only",has name "Unsupported policy",has cx_created == $now,has cx_tax_bp 0,has cx_discount_bp 0,has cx_shipping 0,has cx_enabled true,has cx_instalments false,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "month";(merchant:$t,pricing:$p) isa commerce_pricing_owner;fetch {"id":$p.cx_id};"#;
        ensure!(write(&adapter,&seller,unsupported.into()).await.is_err(),"Published currency unsupported by selected plugin");
        let missing_currency=unsupported.replace("has cx_currency \"JPY\",", "").replace("has cx_plugin \"usd-only\",", "").replace("unsupported-policy","missing-currency");
        ensure!(write(&adapter,&seller,missing_currency).await.is_err(),"New pricing omitted its currency");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query(r#"match $c isa bill_customer,has bill_key "customer";$o isa commerce_order,has cx_id "quote-JPY";update $c has bill_method_plugin "bank",has bill_payment_method "method";insert $m isa bill_saved_method,has bill_key "method-fact",has method_plugin "bank",has method_ref "method",has method_automatic true,has method_active true,has method_pending false,has method_verified_at 2026-11-18T00:00:00,has method_currency "USD";$p isa bill_plan,has bill_key "yen-plan",has name "Yen plan",has bill_amount 1001,has bill_due_days 0,has bill_anchor 0,has bill_next 0,has bill_sequence 0,has bill_interval "month",has bill_status "active";(customer:$c,plan:$p) isa bill_plan_owner;(order:$o,plan:$p) isa commerce_execution;(customer:$c,method:$m) isa bill_method_owner;"#).await?;tx.commit().await?;
        let tx=driver.transaction(&db,TransactionType::Read).await?;
        let method=database::documents(&tx,"match $c isa bill_customer,has bill_key 'customer';$p isa bill_plan,has bill_key 'yen-plan';let $supported=bill_method_supports_plan($c,$p);fetch {'supported':$supported};").await?;
        ensure!(method.len()==1&&method[0]["supported"]==false,"USD-only saved method accepted a JPY plan");tx.close().await?;
        mixed_checkout_policies(&adapter,&buyer,&seller,&outsider).await.context("Mixed native policy regression")?;
        configuration_policies(&adapter,&buyer,&seller,&outsider,&driver,&db).await.context("Configuration native policies")?;
        write(&adapter,&seller,"match $f isa commerce_offer,has cx_id 'item';update $f has cx_enabled false;fetch {'id':$f.cx_id};".into()).await?;
        ensure!(write(&adapter,&buyer,order("withdrawn","item",1001,1,("once","once",1,0))).await.is_err(),"Withdrawn offer remained purchasable");
        let history=adapter.read(&buyer,json!({"query":"match $o isa commerce_order,has cx_id 'purchase';$f isa commerce_offer;(order:$o,offer:$f) isa commerce_line;fetch {'offer':$f.cx_id};"})).await?;
        ensure!(history["ok"]["answers"].as_array().context("Historical terms")?.len()==1,"Withdrawing publication hid accepted terms");
        let tx=driver.transaction(&db,TransactionType::Write).await?;
        tx.query("match $u isa person,has identity_id 'buyer-owner';$m isa membership,links(member:$u);update $m has membership_role 'member';").await?;tx.commit().await?;
        ensure!(write(&adapter,&buyer,order("revoked","monthly",2500,1,("recurring","month",0,0))).await.is_err(),"Revoked buyer retained checkout authority");
        Ok::<_,anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&db).await?.delete().await?;
    } else {
        eprintln!("Retained commerce fixture: {db}");
    }
    result
}

#[tokio::test]
#[ignore = "Explicit private development checkpoint; restores only new shadows, no providers"]
async fn checkout_checkpoint_preserves_full_graph_and_existing_approvals() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let directory = std::path::PathBuf::from(std::env::var("BUSINESS_CHECKOUT_CHECKPOINT")?);
    let source = std::env::var("BUSINESS_CHECKOUT_SOURCE")?;
    ensure!(directory.is_absolute() && database::development_version(&source).is_some());
    let driver = server::recovery::local_driver().await?;
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Checkout checkpoint rehearsal target: {target}");
    let count =
        server::commerce::migration::import(&driver, &driver, &source, &target, &directory).await?;
    ensure!(count > 0, "Empty checkpoint cannot prove preservation");
    println!("Preserved {count} original objects, all attributes, relation roles and existing plan approvals; source and checkpoint untouched, no provider calls");
    Ok(())
}
fn mixed_quote(id: &str) -> String {
    format!(
        r#"match $c isa bill_customer,has bill_key "customer";$p isa commerce_pricing,has cx_id "mixed-policy";
      $f isa commerce_offer,has cx_id "item";$m isa commerce_offer,has cx_id "monthly";let $now=request_now();
      insert $checkout isa commerce_checkout,has cx_id "{id}",has cx_created == $now,has cx_currency "USD",has cx_groups 2,has cx_request_hash "reviewed-cart";
      (checkout:$checkout,customer:$c,pricing:$p) isa commerce_checkout_owner;
      $once isa commerce_order,has cx_id "{id}-once",has cx_created == $now,has cx_currency "USD",has cx_lines 1,has cx_mode "once",has cx_interval "once",has cx_count 1,has cx_deposit 0;
      (order:$once,customer:$c,pricing:$p) isa commerce_order_owner;
      $line1 isa commerce_line,links(order:$once,offer:$f),has cx_id "{id}-line1",has cx_created == $now,has cx_unit 1001,has cx_quantity 1;
      $monthly isa commerce_order,has cx_id "{id}-month",has cx_created == $now,has cx_currency "USD",has cx_lines 1,has cx_mode "recurring",has cx_interval "month",has cx_count 0,has cx_deposit 0;
      (order:$monthly,customer:$c,pricing:$p) isa commerce_order_owner;
      $line2 isa commerce_line,links(order:$monthly,offer:$m),has cx_id "{id}-line2",has cx_created == $now,has cx_unit 2500,has cx_quantity 1;
      (checkout:$checkout,order:$once) isa commerce_checkout_item;(checkout:$checkout,order:$monthly) isa commerce_checkout_item;
      fetch {{"id":$checkout.cx_id}};"#
    )
}
async fn mixed_checkout_policies(
    adapter: &SchemaPolicyAdapter,
    buyer: &AuthenticatedScope,
    seller: &AuthenticatedScope,
    outsider: &AuthenticatedScope,
) -> Result<()> {
    write(adapter,seller,r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();
      insert $p isa commerce_pricing,has cx_id "mixed-policy",has cx_currency "USD",has cx_plugin "usd-only",has name "Per-schedule pricing",has cx_created == $now,has cx_tax_bp 500,has cx_discount_bp 1000,has cx_shipping 100,has cx_enabled true,has cx_instalments true,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "month";
      (merchant:$t,pricing:$p) isa commerce_pricing_owner;fetch {"id":$p.cx_id};"#.into()).await?;
    write(adapter, buyer, mixed_quote("mixed"))
        .await
        .context("Atomic mixed quote")?;
    let q = include_str!("../../public/queries/data/commerce-checkouts.tql");
    for (scope, expected) in [(buyer, 1), (seller, 1), (outsider, 0)] {
        let r = adapter.read(scope, json!({"query":q})).await?;
        ensure!(
            r["ok"]["answers"]
                .as_array()
                .context("Checkout projection")?
                .len()
                == expected,
            "Checkout tenant isolation failed"
        );
        if expected == 1 {
            ensure!(
                r["ok"]["answers"][0]["data"]["groups"]
                    .as_array()
                    .context("Groups")?
                    .len()
                    == 2
            );
        }
    }
    let partial = r#"match $checkout isa commerce_checkout,has cx_id "mixed";$o isa commerce_order,has cx_id "mixed-once";let $actor=request_identity();let $now=request_now();insert (checkout:$checkout,actor:$actor) isa commerce_checkout_signatory,has cx_created == $now;(order:$o,actor:$actor) isa commerce_signatory,has cx_created == $now;fetch {"accepted":true};"#;
    ensure!(
        write(adapter, buyer, partial.into()).await.is_err(),
        "Accepted only part of the reviewed checkout"
    );
    let individual = r#"match $o isa commerce_order,has cx_id "mixed-once";let $actor=request_identity();let $now=request_now();insert (order:$o,actor:$actor) isa commerce_signatory,has cx_created == $now;fetch {"accepted":true};"#;
    ensure!(
        write(adapter, buyer, individual.into()).await.is_err(),
        "Single-order path bypassed aggregate acceptance"
    );
    let accepted=adapter.read(buyer,json!({"query":"match $o isa commerce_order,has cx_id 'mixed-once';fetch {'accepted':request_commerce_accepted($o)};"})).await?;
    ensure!(
        accepted["ok"]["answers"][0]["data"]["accepted"] == false,
        "Denied partial acceptance left a signature"
    );
    let full=partial.replace("let $actor=request_identity();","$other isa commerce_order,has cx_id 'mixed-month';let $actor=request_identity();").replace("fetch {\"accepted\":true};","(order:$other,actor:$actor) isa commerce_signatory,has cx_created == $now;fetch {\"accepted\":true};");
    ensure!(
        write(adapter, seller, full.clone()).await.is_err(),
        "Seller accepted a buyer's obligations"
    );
    write(adapter, buyer, full)
        .await
        .context("Atomic complete acceptance")?;
    ensure!(
        write(
            adapter,
            buyer,
            mixed_quote("missing-group").replace("has cx_groups 2", "has cx_groups 3")
        )
        .await
        .is_err(),
        "Accepted incomplete group membership"
    );
    ensure!(
        write(
            adapter,
            buyer,
            mixed_quote("wrong-denomination").replacen(
                "has cx_currency \"USD\"",
                "has cx_currency \"JPY\"",
                1
            )
        )
        .await
        .is_err(),
        "Cross-currency checkout accepted"
    );
    let duplicate = mixed_quote("duplicate-cadence")
        .replace(
            "$f isa commerce_offer,has cx_id \"item\"",
            "$f isa commerce_offer,has cx_id \"monthly\"",
        )
        .replace(
            "has cx_mode \"once\",has cx_interval \"once\",has cx_count 1",
            "has cx_mode \"recurring\",has cx_interval \"month\",has cx_count 0",
        )
        .replace("has cx_unit 1001", "has cx_unit 2500");
    ensure!(
        write(adapter, buyer, duplicate).await.is_err(),
        "Duplicate cadence created duplicate obligations"
    );
    write(
        adapter,
        buyer,
        order("late-group", "item", 1001, 1, ("once", "once", 1, 0)),
    )
    .await?;
    ensure!(write(adapter,buyer,"match $checkout isa commerce_checkout,has cx_id 'mixed';$o isa commerce_order,has cx_id 'late-group';insert $l isa commerce_checkout_item,links(checkout:$checkout,order:$o);fetch {'ok':true};".into()).await.is_err(),"Grafted another order after acceptance");
    Ok(())
}
#[tokio::test]
#[ignore = "Private development checkpoint; loopback verifier only; disposable source/successor copies, no providers"]
async fn retired_generation_is_fenced_by_native_rbac_and_remains_recoverable() -> Result<()> {
    dotenvy::from_path(std::env::var("BUSINESS_ENV_FILE")?)?;
    ensure!(std::env::var("TYPEDB_DB")? == "business_dev");
    let driver = Arc::new(server::recovery::local_driver().await?);
    let checkpoint = std::path::PathBuf::from(std::env::var("BUSINESS_CHECKOUT_CHECKPOINT")?);
    let original = std::env::var("BUSINESS_CHECKOUT_SOURCE")?;
    ensure!(database::development_version(&original).is_some());
    let version = 5000 + (uuid::Uuid::new_v4().as_u128() % 2000) as u32 * 2;
    let source = format!("business_native_dev_v{version}");
    let target = format!("business_native_dev_v{}", version + 1);
    ensure!(
        !driver.databases().contains(&source).await?
            && !driver.databases().contains(&target).await?,
        "Generated fixture exists; overwrite nothing"
    );
    let directory = tempfile::tempdir()?;
    for suffix in ["records.json", "graph.json"] {
        std::fs::copy(
            checkpoint.join(format!("{original}.{suffix}")),
            directory.path().join(format!("{source}.{suffix}")),
        )?;
    }
    let data = checkpoint.join(format!("{original}.typedb"));
    driver
        .databases()
        .import_from_file(
            &source,
            std::fs::read_to_string(checkpoint.join(format!("{original}.tql")))?,
            &data,
        )
        .await?;
    driver
        .databases()
        .import_from_file(&target, database::import_schema(), &data)
        .await?;
    let result=async{
        let tx=driver.transaction(&source,TransactionType::Read).await?;
        let rows=database::documents(&tx,&format!("match $t isa tenant;$i isa identity;let $allowed=role_at_least($i,$t,'owner',{});$allowed==true;select $t,$i;limit 1;fetch {{'tenant':$t.tenant_id,'identity':$i.identity_id,'name':$t.name}};",server::clock::native_utc())).await?;tx.close().await?;
        ensure!(rows.len()==1,"Checkpoint needs an active owner for the fence proof");
        let scope=AuthenticatedScope::new(rows[0]["tenant"].as_str().context("Tenant")?,rows[0]["identity"].as_str().context("Identity")?)?;
        let source_adapter=SchemaPolicyAdapter::new(Arc::new(TypeDBState{driver:driver.clone(),database:source.clone()}));
        let target_adapter=SchemaPolicyAdapter::new(Arc::new(TypeDBState{driver:driver.clone(),database:target.clone()}));
        let visible=|value:&Value|value["ok"]["answers"].as_array().map_or(0,Vec::len);
        let query=json!({"query":"match $t isa tenant;fetch {'id':$t.tenant_id};"});
        ensure!(visible(&source_adapter.read(&scope,query.clone()).await?)>0);
        let tenant=serde_json::to_string(&rows[0]["tenant"])?;let original_name=serde_json::to_string(&rows[0]["name"])?;
        let tx=driver.transaction(&target,TransactionType::Write).await?;tx.query(format!("match $t isa tenant,has tenant_id {tenant};update $t has name 'Unverified successor';")).await?;tx.commit().await?;
        ensure!(server::commerce::migration::retire_source(&driver,&source,&target,directory.path()).await.is_err(),"Retired against an unverified successor");
        ensure!(visible(&source_adapter.read(&scope,query.clone()).await?)>0,"Failed preflight changed source access");
        let tx=driver.transaction(&target,TransactionType::Write).await?;tx.query(format!("match $t isa tenant,has tenant_id {tenant};update $t has name {original_name};")).await?;tx.commit().await?;
        let report=server::commerce::migration::retire_source(&driver,&source,&target,directory.path()).await?;
        ensure!(visible(&source_adapter.read(&scope,query.clone()).await?)==0,"Retired source retained tenant access");
        ensure!(visible(&target_adapter.read(&scope,query).await?)>0,"Retirement deactivated the successor");
        let denied=source_adapter.write(&scope,json!({"query":format!("match $t isa tenant,has tenant_id {tenant};update $t has revision 'retirement-bypass';fetch {{'id':$t.tenant_id}};")})).await;
        ensure!(denied.is_err()||visible(&denied?)==0,"Retired source accepted an application write");
        ensure!(server::commerce::migration::retire_source(&driver,&source,&target,directory.path()).await?==report,"Retirement is not safely repeatable");
        // A rollback can restore only the recorded original active set. No worker
        // runs on either disposable fixture and no payment state is changed.
        let intent:Value=serde_json::from_slice(&std::fs::read(directory.path().join(format!("retire-{source}.intent.json")))?)?;
        let choices=intent["tenants"].as_array().context("Original active set")?.iter().map(|id|format!("{{$id=={id};}}")).collect::<Vec<_>>().join("or");
        let tx=driver.transaction(&source,TransactionType::Write).await?;tx.query(format!("match $t isa tenant,has tenant_id $id;{choices};update $t has status 'active';")).await?;tx.commit().await?;
        let expected:std::collections::BTreeMap<String,usize>=serde_json::from_slice(&std::fs::read(directory.path().join(format!("{source}.graph.json")))?)?;
        ensure!(database::snapshot::graph(&driver,&source,&[]).await?==expected&&database::snapshot::graph(&driver,&target,&[]).await?==expected,"Fence/rollback changed original work");
        println!("Verified native source read/write denial, unchanged successor access, safe retry, full graph preservation and exact rollback without providers");
        Ok::<_,anyhow::Error>(())
    }.await;
    if result.is_ok() {
        driver.databases().get(&source).await?.delete().await?;
        driver.databases().get(&target).await?.delete().await?;
    } else {
        eprintln!("Retained retirement fixtures {source} and {target}");
    }
    result
}

fn configuration_publish(id: &str, previous: Option<&str>, max_quantity: i64) -> String {
    let spec: Value =
        serde_json::from_str(include_str!("../../public/checkout-configuration.json")).unwrap();
    let mut fields = spec["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            format!(
                "has {} {}",
                f["attribute"].as_str().unwrap(),
                if f["key"] == "max_quantity" {
                    json!(max_quantity)
                } else {
                    f["default"].clone()
                }
            )
        })
        .collect::<Vec<_>>();
    for (key, attribute) in [
        ("currencies", "cx_currency"),
        ("modes", "cx_mode"),
        ("kinds", "cx_kind"),
    ] {
        for value in spec[key].as_array().unwrap() {
            fields.push(format!("has {attribute} {value}"));
        }
    }
    let messages=spec["messages"].as_object().unwrap().iter().enumerate().map(|(i,(kind,m))|format!("$m{i} isa commerce_message,has cx_id '{id}:{kind}',has cfg_message_kind '{kind}',has cfg_message_subject {},has cfg_message_text {},has cfg_message_enabled {}; (configuration:$cfg,message:$m{i}) isa commerce_message_owner;",m["subject"],m["text"],m["enabled"])).collect::<String>();
    let (head_match,head_insert,update)=match previous {None=>("not {(merchant:$t) isa commerce_configuration_head_owner;};".into(),format!("$h isa commerce_configuration_head,has cx_id 'seller',has cfg_active '{id}';(merchant:$t,head:$h) isa commerce_configuration_head_owner;"),String::new()),Some(old)=>(format!("$h isa commerce_configuration_head,has cfg_active '{old}';(merchant:$t,head:$h) isa commerce_configuration_head_owner;"),String::new(),format!("update $h has cfg_active '{id}';"))};
    format!("match $t isa tenant,has tenant_id 'seller';{head_match}let $now=request_now();let $actor=request_identity();insert $cfg isa commerce_configuration,has cx_id '{id}',has cx_created == $now,has cfg_hash '{}',{};(merchant:$t,configuration:$cfg) isa commerce_configuration_owner;{messages}{head_insert}(head:$h,configuration:$cfg,actor:$actor) isa commerce_configuration_activation,has cx_id '{id}:activation',has cx_created == $now;{update}fetch {{'id':$cfg.cx_id}};","a".repeat(64),fields.join(","))
}
async fn configuration_policies(
    adapter: &SchemaPolicyAdapter,
    buyer: &AuthenticatedScope,
    seller: &AuthenticatedScope,
    outsider: &AuthenticatedScope,
    driver: &typedb_driver::TypeDBDriver,
    db: &str,
) -> Result<()> {
    ensure!(
        write(
            adapter,
            buyer,
            configuration_publish("buyer-profile", None, 2)
        )
        .await
        .is_err(),
        "Buyer published merchant configuration"
    );
    ensure!(
        write(
            adapter,
            outsider,
            configuration_publish("foreign-profile", None, 2)
        )
        .await
        .is_err(),
        "Foreign tenant published merchant configuration"
    );
    let bad_brand = configuration_publish("bad-brand", None, 2).replace(
        "has cfg_brand \"\"",
        &format!(
            "has cfg_brand {}",
            serde_json::to_string("Unsafe\r\nHeader")?
        ),
    );
    ensure!(
        write(adapter, seller, bad_brand).await.is_err(),
        "Direct data client inserted a multiline notification name"
    );
    let bad_template = configuration_publish("bad-template", None, 2).replace(
        "has cfg_message_subject \"Payment received\"",
        "has cfg_message_subject \"{credentials}\"",
    );
    ensure!(
        write(adapter, seller, bad_template).await.is_err(),
        "Direct data client inserted an unknown template variable"
    );
    write(
        adapter,
        seller,
        configuration_publish("config-one", None, 2),
    )
    .await
    .context("Publish native checkout configuration")?;
    let configured = |id: &str, quantity: i64, profile: &str| {
        order(id,"item",1001,quantity,("once","once",1,0)).replace("has cx_id \"standard\"","has cx_id \"mixed-policy\"").replace("let $now=request_now();",&format!("$cfg isa commerce_configuration,has cx_id '{profile}';let $now=request_now();")).replace("fetch {\"id\":$o.cx_id};","(order:$o,configuration:$cfg) isa commerce_configuration_snapshot;fetch {\"id\":$o.cx_id};")
    };
    ensure!(
        write(
            adapter,
            buyer,
            order(
                "missing-configuration",
                "item",
                1001,
                1,
                ("once", "once", 1, 0)
            )
        )
        .await
        .is_err(),
        "Omitting configuration bypassed tenant rules"
    );
    ensure!(
        write(
            adapter,
            buyer,
            configured("excess-config-quantity", 3, "config-one")
        )
        .await
        .is_err(),
        "Tenant quantity cap bypassed"
    );
    write(
        adapter,
        buyer,
        configured("configured-accepted", 2, "config-one"),
    )
    .await?;
    write(
        adapter,
        buyer,
        configured("configured-stale", 1, "config-one"),
    )
    .await?;
    let accept = |id: &str| {
        format!("match $o isa commerce_order,has cx_id '{id}';let $actor=request_identity();let $now=request_now();insert (order:$o,actor:$actor) isa commerce_signatory,has cx_created == $now;fetch {{'id':$o.cx_id}};")
    };
    write(adapter, buyer, accept("configured-accepted")).await?;
    write(
        adapter,
        seller,
        configuration_publish("config-two", Some("config-one"), 1),
    )
    .await?;
    ensure!(
        write(adapter, buyer, accept("configured-stale"))
            .await
            .is_err(),
        "Stale quote was accepted after configuration changed"
    );
    ensure!(
        write(
            adapter,
            buyer,
            configured("stale-config-create", 1, "config-one")
        )
        .await
        .is_err(),
        "New quote pinned an old configuration"
    );
    write(
        adapter,
        buyer,
        configured("current-config-create", 1, "config-two"),
    )
    .await?;
    for query in ["match $p isa commerce_configuration,has cx_id 'config-one';update $p has cfg_max_quantity 60;fetch {'id':$p.cx_id};","match $h isa commerce_configuration_head;update $h has cfg_active 'config-one';fetch {'id':$h.cx_id};","match $m isa commerce_message,has cx_id 'config-one:receipt';update $m has cfg_message_enabled false;fetch {'id':$m.cx_id};"] {ensure!(write(adapter,seller,query.into()).await.is_err(),"Immutable configuration or unaudited activation was accepted");}
    let foreign = adapter
        .read(
            outsider,
            json!({"query":"match $p isa commerce_configuration;fetch {'id':$p.cx_id};"}),
        )
        .await?;
    ensure!(
        foreign["ok"]["answers"]
            .as_array()
            .context("Configuration scope")?
            .is_empty(),
        "Configuration leaked across tenants"
    );
    let history=adapter.read(buyer,json!({"query":"match $o isa commerce_order,has cx_id 'configured-accepted';(order:$o,configuration:$p) isa commerce_configuration_snapshot;$p isa commerce_configuration;fetch {'id':$p.cx_id,'limit':$p.cfg_max_quantity};"})).await?;
    ensure!(
        history["ok"]["answers"][0]["data"]["id"] == "config-one"
            && history["ok"]["answers"][0]["data"]["limit"].as_f64() == Some(2.0),
        "Accepted order configuration was reinterpreted: {history}"
    );
    // Restore the original cap through an audited activation; no accepted order is rewritten.
    write(adapter,seller,"match $h isa commerce_configuration_head,has cfg_active 'config-two';$p isa commerce_configuration,has cx_id 'config-one';let $actor=request_identity();let $now=request_now();insert (head:$h,configuration:$p,actor:$actor) isa commerce_configuration_activation,has cx_id 'restore-config-one',has cx_created == $now;update $h has cfg_active 'config-one';fetch {'id':$h.cx_id};".into()).await?;
    let tx = driver.transaction(db, TransactionType::Write).await?;
    tx.query("match $t isa tenant,has tenant_id 'seller';$owner isa person,has identity_id 'seller-owner';insert $editor isa person,has identity_id 'checkout-editor',has name 'Checkout editor',has status 'active';(tenant:$t,member:$editor) isa membership,has membership_role 'member',has start_at 2020-01-01T00:00:00;$resource isa resource,has resource_id 'checkout-configuration',has name 'Checkout configuration',has status 'active';(tenant:$t,resource:$resource) isa resource_ownership;$grant isa exception_access_grant,links(tenant:$t,grantee:$editor,resource:$resource,grantor:$owner),has grant_id 'checkout-editor-grant',has permission_level 'write',has effect 'allow',has start_at 2020-01-01T00:00:00,has reason 'Configuration editor fixture';").await?;
    tx.commit().await?;
    let editor = AuthenticatedScope::new("seller", "checkout-editor")?;
    write(
        adapter,
        &editor,
        configuration_publish("config-delegated", Some("config-one"), 1),
    )
    .await
    .context("Native delegated checkout editor")?;
    let tx = driver.transaction(db, TransactionType::Write).await?;
    tx.query("match $g isa exception_access_grant,has grant_id 'checkout-editor-grant';update $g has end_at 2020-01-02T00:00:00;").await?;
    tx.commit().await?;
    ensure!(
        write(
            adapter,
            &editor,
            configuration_publish("config-revoked", Some("config-delegated"), 1)
        )
        .await
        .is_err(),
        "Revoked native configuration grant remained usable"
    );
    let pin = |query: String, profile: &str| {
        query.replace("let $now=request_now();",&format!("$cfg isa commerce_configuration,has cx_id '{profile}';let $now=request_now();")).replace("fetch {\"id\":$o.cx_id};","(order:$o,configuration:$cfg) isa commerce_configuration_snapshot;fetch {\"id\":$o.cx_id};")
    };
    let two_lines = |id: &str| {
        order(id,"item",1001,1,("once","once",1,0)).replace("has cx_lines 1","has cx_lines 2").replace("fetch {\"id\":$o.cx_id};",&format!("$extra isa commerce_line,links(order:$o,offer:$f),has cx_id '{id}:extra',has cx_created == $now,has cx_unit 1001,has cx_quantity 1;fetch {{\"id\":$o.cx_id}};"))
    };
    for query in [
        order(
            "allowed-recurring",
            "monthly",
            2500,
            1,
            ("recurring", "month", 0, 0),
        ),
        order(
            "allowed-donation",
            "donation",
            1234,
            1,
            ("once", "once", 1, 0),
        ),
        order("allowed-eur", "item-EUR", 1001, 1, ("once", "once", 1, 0))
            .replace("has cx_id \"standard\"", "has cx_id \"policy-EUR\"")
            .replace("has cx_currency \"USD\"", "has cx_currency \"EUR\""),
        two_lines("allowed-lines"),
    ] {
        write(adapter, buyer, pin(query, "config-delegated"))
            .await
            .context("Valid baseline before tenant restrictions")?;
    }
    let pin_groups = |id: &str, profile: &str| {
        mixed_quote(id).replace("let $now=request_now();",&format!("$cfg isa commerce_configuration,has cx_id '{profile}';let $now=request_now();")).replace("fetch {\"id\":$checkout.cx_id};","(order:$once,configuration:$cfg) isa commerce_configuration_snapshot;(order:$monthly,configuration:$cfg) isa commerce_configuration_snapshot;fetch {\"id\":$checkout.cx_id};")
    };
    write(
        adapter,
        buyer,
        pin_groups("allowed-config-groups", "config-delegated"),
    )
    .await
    .context("Valid baseline before tenant group restriction")?;
    let mut limited = configuration_publish("config-limited", Some("config-delegated"), 1)
        .replace("has cfg_max_lines 60", "has cfg_max_lines 1");
    for value in ["EUR", "GBP", "CAD", "AUD", "JPY"] {
        limited = limited.replace(&format!(",has cx_currency \"{value}\""), "");
    }
    for (attribute, value) in [
        ("cx_mode", "recurring"),
        ("cx_mode", "instalments"),
        ("cx_kind", "service"),
        ("cx_kind", "donation"),
    ] {
        limited = limited.replace(&format!(",has {attribute} \"{value}\""), "");
    }
    write(adapter, seller, limited).await?;
    for query in [
        order(
            "denied-recurring",
            "monthly",
            2500,
            1,
            ("recurring", "month", 0, 0),
        ),
        order(
            "denied-donation",
            "donation",
            1234,
            1,
            ("once", "once", 1, 0),
        ),
        order("denied-eur", "item-EUR", 1001, 1, ("once", "once", 1, 0))
            .replace("has cx_id \"standard\"", "has cx_id \"policy-EUR\"")
            .replace("has cx_currency \"USD\"", "has cx_currency \"EUR\""),
        two_lines("denied-lines"),
    ] {
        ensure!(
            write(adapter, buyer, pin(query, "config-limited"))
                .await
                .is_err(),
            "Direct data client bypassed a tenant currency, mode, kind or cart-line restriction"
        );
    }
    write(
        adapter,
        seller,
        configuration_publish("config-one-group", Some("config-limited"), 1)
            .replace("has cfg_max_groups 4", "has cfg_max_groups 1")
            .replace(
                "has cfg_quote_window \"15m\"",
                "has cfg_quote_window \"5m\"",
            ),
    )
    .await?;
    let grouped=mixed_quote("configured-groups").replace("let $now=request_now();","$cfg isa commerce_configuration,has cx_id 'config-one-group';let $now=request_now();").replace("fetch {\"id\":$checkout.cx_id};","(order:$once,configuration:$cfg) isa commerce_configuration_snapshot;(order:$monthly,configuration:$cfg) isa commerce_configuration_snapshot;fetch {\"id\":$checkout.cx_id};");
    ensure!(
        write(adapter, buyer, grouped).await.is_err(),
        "Direct data client bypassed the tenant payment-group limit"
    );
    write(
        adapter,
        buyer,
        pin(
            order("expiry-proof", "item", 1001, 1, ("once", "once", 1, 0)),
            "config-one-group",
        ),
    )
    .await?;
    let tx = driver.transaction(db, TransactionType::Read).await?;
    let expiry=database::documents(&tx,"match $o isa commerce_order,has cx_id 'expiry-proof',has cx_created $created;let $before=$created - PT1M;let $within=$created + PT4M;let $after=$created + PT6M;fetch {'future':cfg_quote_fresh($o,$before),'within':cfg_quote_fresh($o,$within),'expired':cfg_quote_fresh($o,$after)};").await?;
    tx.close().await?;
    ensure!(
        expiry.len() == 1
            && expiry[0]["within"] == true
            && expiry[0]["expired"] == false
            && expiry[0]["future"] == false,
        "Native quote-time boundaries failed: {expiry:?}"
    );
    let mut no_refunds = configuration_publish("config-no-refunds", Some("config-one-group"), 1)
        .replace("has cfg_refunds true", "has cfg_refunds false")
        .replace("has cfg_reminder_count 4", "has cfg_reminder_count 0")
        .replace("has cfg_due_days 0", "has cfg_due_days 5");
    no_refunds =
        regex::Regex::new("(has cfg_message_kind 'receipt',[^;]+has cfg_message_enabled )true")?
            .replace(&no_refunds, "${1}false")
            .into_owned();
    write(adapter, seller, no_refunds).await?;
    let policy_order = order(
        "refund-policy-disabled",
        "item",
        1001,
        1,
        ("once", "once", 1, 0),
    )
    .replace("has cx_id \"standard\"", "has cx_id \"mixed-policy\"");
    write(adapter, buyer, pin(policy_order, "config-no-refunds")).await?;
    write(adapter, buyer, accept("refund-policy-disabled")).await?;
    write(
        adapter,
        seller,
        configuration_publish("config-refund-day", Some("config-no-refunds"), 1).replace(
            "has cfg_refund_window \"unlimited\"",
            "has cfg_refund_window \"day\"",
        ),
    )
    .await?;
    let policy_order = order("refund-policy-day", "item", 1001, 1, ("once", "once", 1, 0))
        .replace("has cx_id \"standard\"", "has cx_id \"mixed-policy\"");
    write(adapter, buyer, pin(policy_order, "config-refund-day")).await?;
    write(adapter, buyer, accept("refund-policy-day")).await?;
    // Paid observations here are isolated unit fixtures, never provider settlement overrides.
    let now = server::clock::unix_seconds();
    let tx = driver.transaction(db, TransactionType::Write).await?;
    for id in ["refund-policy-disabled", "refund-policy-day"] {
        tx.query(format!("match $c isa bill_customer,has bill_key 'customer';$o isa commerce_order,has cx_id '{id}';insert $p isa bill_plan,has bill_key '{id}:plan',has name 'Native policy fixture',has bill_amount 1001,has bill_due_days 0,has bill_anchor {now},has bill_next 0,has bill_sequence 1,has bill_interval 'once',has bill_status 'finished';(customer:$c,plan:$p) isa bill_plan_owner;(order:$o,plan:$p) isa commerce_execution;$i isa bill_invoice,has bill_key '{id}:invoice',has name 'Native policy fixture',has bill_amount 1001,has bill_anchor {now},has bill_next {now},has bill_sequence 0,has bill_choice 'manual',has bill_provider 'usd-only',has bill_status 'paid',has bill_external_id 'synthetic-unit-fixture',has bill_url 'https://example.invalid',has bill_updated {now},has bill_revision 'initial';(plan:$p,invoice:$i) isa bill_invoice_owner;")).await?;
    }
    tx.commit().await?;
    ensure!(write(adapter,seller,"match $i isa bill_invoice,has bill_key 'refund-policy-disabled:invoice';let $at=request_now();let $actor=request_identity();insert $r isa commerce_refund,has cx_id 'disabled-refund-proposal',has cx_created == $at,has cx_amount 1,has cx_refund_status 'queued',has cx_busy 'refund-policy-disabled:invoice:refund';(invoice:$i,request:$r,actor:$actor) isa commerce_refund_owner;fetch {'id':$r.cx_id};".into()).await.is_err(),"New tenant settings overrode a disabled accepted refund policy");
    let absent=adapter.read(seller,json!({"query":"match $r isa commerce_refund,has cx_id 'disabled-refund-proposal';fetch {'id':$r.cx_id};"})).await?;
    ensure!(
        absent["ok"]["answers"]
            .as_array()
            .context("Denied refund proposal")?
            .is_empty(),
        "Denied refund left an execution proposal"
    );
    let tx = driver.transaction(db, TransactionType::Read).await?;
    let disabled=database::documents(&tx,"match $i isa bill_invoice,has bill_key 'refund-policy-disabled:invoice';$o isa commerce_order,has cx_id 'refund-policy-disabled',has cx_created $created;let $at=$created;fetch {'refunds':cfg_invoice_refunds($i,$at),'receipt':cfg_notice_enabled($i,'receipt'),'days':cfg_invoice_due_days($o)};").await?;
    ensure!(
        disabled.len() == 1
            && disabled[0]["refunds"] == false
            && disabled[0]["receipt"] == false
            && server::billing::engine::number(&disabled[0], "days")? == 5,
        "Pinned refund/receipt/payment window policy differed: {disabled:?}"
    );
    let window=database::documents(&tx,"match $i isa bill_invoice,has bill_key 'refund-policy-day:invoice';$o isa commerce_order,has cx_id 'refund-policy-day',has cx_created $created;let $at=$created;let $within=$at + PT23H;let $after=$at + P2D;fetch {'within':cfg_invoice_refunds($i,$within),'expired':cfg_invoice_refunds($i,$after)};").await?;
    ensure!(
        window.len() == 1 && window[0]["within"] == true && window[0]["expired"] == false,
        "Native refund-window boundary failed"
    );
    tx.close().await?;
    println!("Verified native configuration ownership, delegation/revocation, limits, immutable snapshots, stale review denial, audited rollback and cross-tenant isolation");
    Ok(())
}
