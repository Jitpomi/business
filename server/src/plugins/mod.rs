pub mod mercury;
pub mod stripe;
// Built-in plugins and composition. Business services depend only on finance contracts.
pub mod builtin_transport;
pub mod configuration;
mod financial;
mod history;
mod notification;
mod signature;
pub use financial::BuiltinFinancial;
pub use notification::Resend;

pub fn routes(
    http: dog_transport::http::DogHttpService<serde_json::Value, crate::BusinessParams>,
) -> axum::Router {
    crate::plugins::mercury::router(http.clone()).merge(crate::plugins::stripe::router(http))
}
