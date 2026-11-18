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
        write(&adapter,&seller,offer("item","product",1001,1001,1001,"once")).await.context("Create product")?;
        write(&adapter,&seller,offer("monthly","service",2500,2500,2500,"month")).await?;
        write(&adapter,&seller,offer("donation","donation",0,100,100000,"once")).await?;
        write(&adapter,&seller,offer("recurring-donation","donation",0,100,100000,"month")).await?;
        ensure!(write(&adapter,&buyer,offer("forged","product",100,100,100,"once")).await.is_err(),"Buyer created a merchant offer");
        write(&adapter,&seller,r#"match $t isa tenant,has tenant_id "seller";let $now=request_now();
insert $p isa commerce_pricing,has cx_id "standard",has name "No adjustments",has cx_created == $now,has cx_tax_bp 0,has cx_discount_bp 0,has cx_shipping 0,has cx_enabled true,has cx_instalments true,has cx_limit 12,has cx_deposit_bp 0,has cx_plan_period "week",has cx_plan_period "month";
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
insert $p isa commerce_pricing,has cx_id "adjusted",has name "Published adjustment quote",has cx_created == $now,
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
    let driver = crate::test_driver::connect(
        &std::env::var("TYPEDB_ADDR")?,
        &std::env::var("TYPEDB_USERNAME")?,
        &std::env::var("TYPEDB_PASSWORD")?,
        true,
    )
    .await?;
    let target = format!("business_backend_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Checkout checkpoint rehearsal target: {target}");
    let count = server::commerce::migration::import(&driver, &source, &target, &directory).await?;
    ensure!(count > 0, "Empty checkpoint cannot prove preservation");
    println!("Preserved {count} original objects, all attributes, relation roles and existing plan approvals; source and checkpoint untouched, no provider calls");
    Ok(())
}
