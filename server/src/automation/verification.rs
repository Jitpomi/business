//! Safe provider-verification outcomes shared by connector implementations.
use dog_core::DogError;

#[derive(Debug)]
pub enum Failure {
    Rejected(u16),
    RateLimited,
    Unavailable,
    ProviderError(u16),
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Connection verification failed")
    }
}
impl std::error::Error for Failure {}

/// Classify transport status without exposing response bodies or credentials.
pub fn response(status: reqwest::StatusCode) -> anyhow::Result<()> {
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(Failure::RateLimited.into())
    } else if status.is_client_error() {
        Err(Failure::Rejected(status.as_u16()).into())
    } else {
        Err(Failure::ProviderError(status.as_u16()).into())
    }
}

/// Only classified failures cross the API boundary. Arbitrary connector errors
/// can contain private URLs, headers or provider response bodies.
pub fn public(error: anyhow::Error) -> anyhow::Error {
    let message = match error.downcast_ref::<Failure>() {
        Some(Failure::Rejected(status)) => return DogError::conflict(format!(
            "The provider rejected connection verification (HTTP {status}). Check its credentials, permissions and network access rules."
        )).into_anyhow(),
        Some(Failure::RateLimited) => return DogError::unavailable(
            "The provider rate-limited verification. Wait before trying again."
        ).into_anyhow(),
        Some(Failure::Unavailable) => return DogError::unavailable(
            "The provider could not be reached within the verification deadline. The connection was not verified."
        ).into_anyhow(),
        Some(Failure::ProviderError(_)) | None => "The provider could not complete verification. The connection was not verified.",
    };
    DogError::bad_gateway(message).into_anyhow()
}
