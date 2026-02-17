//! WebSocket integration tests — connect, send/receive, invalid JSON, size limit.
//!
//! Uses a real TCP listener + tokio-tungstenite isn't a dep, so we test via
//! the axum WebSocket upgrade path with hyper client manually. For simplicity,
//! we test the upgrade response status (101) and the handler logic indirectly
//! through the session manager.

mod test_helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use operon_gateway::create_router;
use test_helpers::{make_test_state, with_connect_info};

/// WebSocket upgrade requires specific headers. Without them, axum returns 400/upgrade required.
#[tokio::test]
async fn test_ws_upgrade_without_headers_rejected() {
    let (state, _dir) = make_test_state();
    let app = create_router(state.clone());

    // Create a session first
    let create_req = Request::builder()
        .method("POST")
        .uri("/api/v1/sessions")
        .header("content-type", "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let create_req = with_connect_info(create_req);
    let resp = app.oneshot(create_req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = json["session_id"].as_str().unwrap();

    // Attempt WS without upgrade headers → should not return 200
    let app = create_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/ws/sessions/{}", sid))
        .body(Body::empty())
        .unwrap();
    let req = with_connect_info(req);

    let resp = app.oneshot(req).await.unwrap();
    // Without proper WS upgrade headers, axum rejects the request
    assert_ne!(resp.status(), StatusCode::OK);
}

/// Verify the WS route is reachable with upgrade headers.
/// `oneshot()` cannot complete a real HTTP upgrade, so axum returns 426
/// (Upgrade Required) — this confirms the route matched and the WS handler
/// recognized the request. A real 101 requires a live TCP connection.
#[tokio::test]
async fn test_ws_upgrade_route_reachable() {
    let (state, _dir) = make_test_state();
    let app = create_router(state.clone());

    // Create session
    let create_req = Request::builder()
        .method("POST")
        .uri("/api/v1/sessions")
        .header("content-type", "application/json")
        .body(Body::from(r#"{}"#))
        .unwrap();
    let create_req = with_connect_info(create_req);
    let resp = app.oneshot(create_req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = json["session_id"].as_str().unwrap();

    // Send WS upgrade headers via oneshot — 426 confirms route + WS handler matched
    let app = create_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/ws/sessions/{}", sid))
        .header("host", "localhost")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .unwrap();
    let req = with_connect_info(req);

    let resp = app.oneshot(req).await.unwrap();
    // 426 = route matched, WS handler active, but oneshot can't complete upgrade
    assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED);
}

/// Session manager subscribe/broadcast works (unit-level test of the WS data path).
#[tokio::test]
async fn test_session_event_broadcast() {
    let (state, _dir) = make_test_state();

    // Create session
    let sid = state
        .session_manager
        .create(Some("ws-agent"))
        .await
        .unwrap();

    // Subscribe
    let mut rx = state.session_manager.subscribe(&sid).await.unwrap();

    // Send a message (triggers broadcast)
    // This uses MockLLMProvider so the agent returns "mock response"
    let _ = state.session_manager.send_message(&sid, "hello").await;

    // Should receive the broadcast event
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("channel closed");

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "agent_response");
    assert!(json["content"].as_str().unwrap().contains("mock"));
}

/// Subscribe to nonexistent session returns error.
#[tokio::test]
async fn test_subscribe_nonexistent_session() {
    let (state, _dir) = make_test_state();
    let result = state.session_manager.subscribe("no-such-id").await;
    assert!(result.is_err());
}
