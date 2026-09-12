use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Domain errors whose HTTP responses deliberately avoid internal details.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("resource not found")]
    NotFound,
    #[error("access forbidden")]
    Forbidden,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("internal application error")]
    Internal,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "The requested resource was not found.",
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "You do not have permission to perform this action.",
            ),
            Self::Conflict(_) => (
                StatusCode::CONFLICT,
                "The requested change conflicts with the current state.",
            ),
            Self::Validation(_) => (StatusCode::BAD_REQUEST, "The request is invalid."),
            Self::PayloadTooLarge(_) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "The request payload is too large.",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "The request could not be completed.",
            ),
        };

        (status, message).into_response()
    }
}
