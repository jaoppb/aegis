//! The plain-text answer of a refused request.

use std::fmt::Display;
use std::time::Duration;

use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// A refused request: a status and a short plain-text reason that holds no secrets, and, for a
/// request that may be repeated later, how long to wait.
pub(super) struct Refusal {
    status: StatusCode,
    reason: String,
    retry_after: Option<Duration>,
}

impl Refusal {
    pub(super) fn new(status: StatusCode, reason: &impl Display) -> Self {
        Self {
            status,
            reason: reason.to_string(),
            retry_after: None,
        }
    }

    /// The same refusal, which asks the caller to wait `wait` before it asks again.
    pub(super) fn retry_after(self, wait: Duration) -> Self {
        Self {
            retry_after: Some(wait),
            ..self
        }
    }
}

/// Whole seconds, rounded up, as the `Retry-After` header counts them.
fn seconds_of(wait: Duration) -> u64 {
    let rounding = u64::from(wait.subsec_nanos() != 0);
    wait.as_secs().saturating_add(rounding)
}

fn waiting(response: Response, wait: Duration) -> Response {
    let mut response = response;
    let seconds = HeaderValue::from(seconds_of(wait));
    response.headers_mut().insert(RETRY_AFTER, seconds);
    response
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let response = (self.status, self.reason).into_response();
        match self.retry_after {
            Some(wait) => waiting(response, wait),
            None => response,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_rounded_up_to_whole_seconds() {
        assert_eq!(seconds_of(Duration::from_millis(1200)), 2);
        assert_eq!(seconds_of(Duration::from_secs(3)), 3);
        assert_eq!(seconds_of(Duration::ZERO), 0);
    }
}
