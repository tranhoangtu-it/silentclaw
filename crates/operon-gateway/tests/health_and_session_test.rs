//! Tests for health endpoint and session CRUD (create, get, list, delete, messages).

mod test_helpers;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use operon_gateway::create_router;
use test_helpers::{make_test_state, with_connect_info};

/// Helper: build a request and call the router, return (status, body_bytes).
async fn call(method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Vec<u8>) {
    let (state, _dir) = make_test_state();
    let app = create_router(state);

    let mut builder = Request::builder().method(method).uri(uri);
    let req = if let Some(json) = body {
        builder = builder.header("content-type", "application/json");
        builder.body(Body::from(json.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let req = with_connect_info(req);

    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// Stateful helper that reuses one AppState across multiple requests.
struct TestApp {
    state: operon_gateway::AppState,
    _dir: tempfile::TempDir,
}

impl TestApp {
    fn new() -> Self {
        let (state, _dir) = make_test_state();
        Self { state, _dir }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Vec<u8>) {
        let app = create_router(self.state.clone());
        let mut builder = Request::builder().method(method).uri(uri);
        let req = if let Some(json) = body {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(json.to_string())).unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let req = with_connect_info(req);
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes)
    }
}

// ── Health ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_returns_ok() {
    let (status, body) = call("GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
}

// ── Session CRUD ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_session() {
    let app = TestApp::new();
    let (status, body) = app
        .call("POST", "/api/v1/sessions", Some(r#"{"agent_id":"test"}"#))
        .await;

    assert_eq!(status, StatusCode::CREATED);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["session_id"].is_string());
    assert_eq!(json["agent_name"], "test");
    assert_eq!(json["message_count"], 0);
}

#[tokio::test]
async fn test_get_session() {
    let app = TestApp::new();

    // Create
    let (_, body) = app
        .call("POST", "/api/v1/sessions", Some(r#"{"agent_id":"a1"}"#))
        .await;
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = created["session_id"].as_str().unwrap();

    // Get
    let uri = format!("/api/v1/sessions/{}", sid);
    let (status, body) = app.call("GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["session_id"], sid);
    assert_eq!(json["agent_name"], "a1");
}

#[tokio::test]
async fn test_get_nonexistent_session() {
    let (status, _) = call("GET", "/api/v1/sessions/no-such-id", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_list_sessions() {
    let app = TestApp::new();

    // Create two sessions
    app.call("POST", "/api/v1/sessions", Some(r#"{"agent_id":"s1"}"#))
        .await;
    app.call("POST", "/api/v1/sessions", Some(r#"{"agent_id":"s2"}"#))
        .await;

    let (status, body) = app.call("GET", "/api/v1/sessions", None).await;
    assert_eq!(status, StatusCode::OK);

    let list: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn test_delete_session() {
    let app = TestApp::new();

    // Create
    let (_, body) = app.call("POST", "/api/v1/sessions", Some(r#"{}"#)).await;
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = created["session_id"].as_str().unwrap();

    // Delete
    let uri = format!("/api/v1/sessions/{}", sid);
    let (status, _) = app.call("DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify gone
    let (status, _) = app.call("GET", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_send_message_success() {
    let app = TestApp::new();

    // Create session
    let (_, body) = app.call("POST", "/api/v1/sessions", Some(r#"{}"#)).await;
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = created["session_id"].as_str().unwrap();

    // Send normal message (mock provider returns "mock response")
    let uri = format!("/api/v1/sessions/{}/messages", sid);
    let payload = r#"{"content":"hello"}"#;
    let (status, body) = app.call("POST", &uri, Some(payload)).await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["session_id"], sid);
    assert!(json["content"].as_str().unwrap().contains("mock"));
}

#[tokio::test]
async fn test_message_too_large() {
    let app = TestApp::new();

    // Create session
    let (_, body) = app.call("POST", "/api/v1/sessions", Some(r#"{}"#)).await;
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sid = created["session_id"].as_str().unwrap();

    // Send 51KB message
    let big_content = "x".repeat(51_000);
    let payload = serde_json::json!({ "content": big_content }).to_string();
    let uri = format!("/api/v1/sessions/{}/messages", sid);
    let (status, _) = app.call("POST", &uri, Some(&payload)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
