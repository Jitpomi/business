//! Bounded privileged transactions, available only to server-owned business templates.
use crate::{database::documents, typedb::TypeDBState};
use anyhow::{ensure, Result};
use serde_json::Value;
use std::time::Duration;
use typedb_driver::{TransactionOptions, TransactionType};
const DEADLINE: Duration = Duration::from_secs(30);
pub async fn read(state: &TypeDBState, query: &str) -> Result<Vec<Value>> {
    tokio::time::timeout(DEADLINE, async {
        let tx = state
            .driver
            .transaction_with_options(
                &state.database,
                TransactionType::Read,
                TransactionOptions::new().transaction_timeout(DEADLINE),
            )
            .await?;
        let result = documents(&tx, query).await;
        tx.close().await?;
        result
    })
    .await?
}
pub async fn write(state: &TypeDBState, query: &str) -> Result<Value> {
    let at = query
        .rfind("fetch ")
        .ok_or_else(|| anyhow::anyhow!("Missing write projection"))?;
    // Every template retains $t through select. Updating its revision makes concurrent
    // claims on the same tenant conflict, including phantom/absence checks.
    let query = format!(
        "{} update $t has revision \"{}\"; {}",
        &query[..at],
        uuid::Uuid::new_v4(),
        &query[at..]
    );
    write_projected(state, &query).await
}
/// Queue ledger writes are fenced by the job revision and unique key. They must
/// not mutate the tenant's business fence while a payment journals its outcome.
pub(crate) async fn write_queue(state: &TypeDBState, query: &str) -> Result<Value> {
    write_projected(state, query).await
}
async fn write_projected(state: &TypeDBState, query: &str) -> Result<Value> {
    // Only an explicit server-aborted isolation conflict permits re-running a
    // database transaction. Provider calls never occur inside these templates.
    // One overall deadline bounds all attempts; uncertain commit acknowledgements
    // are returned immediately and must be reconciled by their durable journal.
    tokio::time::timeout(DEADLINE, retry_aborted(|| write_once(state, query))).await?
}
async fn retry_aborted<T, F, Fut>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    for attempt in 0..3 {
        match operation().await {
            Err(error) if definitely_aborted(&error) && attempt < 2 => continue,
            result => return result,
        }
    }
    unreachable!()
}
fn definitely_aborted(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<typedb_driver::Error>()
        .is_some_and(|e| e.code() == "STC2")
}
async fn write_once(state: &TypeDBState, query: &str) -> Result<Value> {
    let tx = state
        .driver
        .transaction_with_options(
            &state.database,
            TransactionType::Write,
            TransactionOptions::new().transaction_timeout(DEADLINE),
        )
        .await?;
    let result = documents(&tx, query).await;
    let rows = match result {
        Ok(rows) if rows.len() == 1 => rows,
        result => {
            tx.close().await?;
            let rows = result?;
            ensure!(
                !rows.is_empty(),
                dog_core::DogError::conflict("The record changed or is no longer available")
                    .into_anyhow()
            );
            ensure!(
                rows.len() == 1,
                "Ambiguous business claim; nothing committed"
            );
            unreachable!()
        }
    };
    // A lost acknowledgement can conceal success. Only typed STC2 explicitly
    // establishes that the whole transaction was aborted by the server.
    tx.commit().await?;
    Ok(rows.into_iter().next().unwrap())
}

#[cfg(test)]
#[path = "../tests/unit/storage.rs"]
mod tests;
