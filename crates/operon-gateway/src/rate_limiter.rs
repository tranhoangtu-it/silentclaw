use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::extract::Request;
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::time::Instant;

/// Simple token bucket rate limiter
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<DashMap<IpAddr, (Instant, u32)>>,
    max_requests_per_minute: u32,
}

impl RateLimiter {
    pub fn new(max_requests_per_minute: u32) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            max_requests_per_minute,
        }
    }

    /// Check if request is allowed for given IP
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        let mut entry = self.buckets.entry(ip).or_insert((now, 0));
        let (last_reset, count) = *entry.value();

        // Reset bucket if window expired
        if now.duration_since(last_reset) >= window {
            *entry.value_mut() = (now, 1);
            true
        } else if count < self.max_requests_per_minute {
            entry.value_mut().1 += 1;
            true
        } else {
            false
        }
    }

    /// Clean up old entries (call periodically)
    pub fn cleanup(&self) {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        self.buckets.retain(|_, (last_reset, _)| {
            now.duration_since(*last_reset) < window
        });
    }
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    rate_limiter: Arc<RateLimiter>,
    request: Request,
    next: Next,
) -> Response {
    let ip = addr.ip();

    if !rate_limiter.check(ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded").into_response();
    }

    next.run(request).await
}
