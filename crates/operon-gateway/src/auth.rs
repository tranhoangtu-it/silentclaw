use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Bearer token authentication state
#[derive(Clone)]
pub struct AuthConfig {
    pub api_token: Option<String>,
}

impl AuthConfig {
    pub fn new(api_token: Option<String>) -> Self {
        Self { api_token }
    }

    pub fn is_enabled(&self) -> bool {
        self.api_token.is_some()
    }
}

/// Authentication middleware for API endpoints
pub async fn auth_middleware(
    auth_config: Arc<AuthConfig>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();

    // Skip auth for health endpoint
    if path == "/health" {
        return next.run(request).await;
    }

    // If auth is disabled, proceed
    if !auth_config.is_enabled() {
        return next.run(request).await;
    }

    // Extract Authorization header
    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if let Some(expected_token) = &auth_config.api_token {
                if token.as_bytes().ct_eq(expected_token.as_bytes()).into() {
                    return next.run(request).await;
                }
            }
        }
        _ => {}
    }

    // Unauthorized
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}
