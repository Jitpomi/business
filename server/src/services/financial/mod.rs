//! Tenant financial-plugin metadata and external operations; account data belongs to /data.
pub mod financial_hooks;
pub mod financial_schema;
pub mod financial_service;
pub mod financial_shared;
pub use financial_service::FinancialService;
