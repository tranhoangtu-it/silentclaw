//! Tests for auth middleware (401 on missing/invalid token) and rate limiter (429).

mod test_helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use operon_gateway::create_router;
use test_helpers::make_auth_test_state;

// ── Auth Middleware ─────────────────────────────────────────────────────

async fn auth_call(token: Option<&str>, state: &operon_gateway::AppState) -> StatusCode {
    let app = create_router(state.clone());
    let mut builder = Request::builder()
        .method("GET")
        .uri("/api/v1/sessions");

    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {}", t));
    }

    let req = builder.body(Body::empty()).unwrap();
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn test_auth_missing_token_returns_401() {
    let state = make_auth_test_state("secret-token");
    let status = auth_call(None, &state).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_invalid_token_returns_401() {
    let state = make_auth_test_state("secret-token");
    let status = auth_call(Some("wrong-token"), &state).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_auth_valid_token_returns_ok() {
    let state = make_auth_test_state("secret-token");
    let status = auth_call(Some("secret-token"), &state).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_health_bypasses_auth() {
    let state = make_auth_test_state("secret-token");
    let app = create_router(state);

    // No token, but /health should still succeed
    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let status = app.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK);
}

// ── Rate Limiter (unit tests on RateLimiter struct directly) ────────────

#[test]
fn test_rate_limiter_allows_within_limit() {
    let limiter = operon_gateway::RateLimiter::new(5);
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

    for _ in 0..5 {
        assert!(limiter.check(ip), "Should allow requests within limit");
    }
}

#[test]
fn test_rate_limiter_blocks_over_limit() {
    let limiter = operon_gateway::RateLimiter::new(3);
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

    // Use up all tokens
    for _ in 0..3 {
        assert!(limiter.check(ip));
    }

    // 4th request should be blocked
    assert!(!limiter.check(ip), "Should block after limit exceeded");
}

#[test]
fn test_rate_limiter_separate_ips() {
    let limiter = operon_gateway::RateLimiter::new(2);
    let ip1: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let ip2: std::net::IpAddr = "10.0.0.2".parse().unwrap();

    // Each IP gets its own bucket
    assert!(limiter.check(ip1));
    assert!(limiter.check(ip1));
    assert!(!limiter.check(ip1)); // blocked

    assert!(limiter.check(ip2)); // different IP, still allowed
    assert!(limiter.check(ip2));
    assert!(!limiter.check(ip2));
}

#[test]
fn test_rate_limiter_cleanup() {
    let limiter = operon_gateway::RateLimiter::new(100);
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

    limiter.check(ip);
    // cleanup should not panic and should retain recent entries
    limiter.cleanup();
}
