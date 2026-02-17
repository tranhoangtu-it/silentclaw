use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Path, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::auth::{auth_middleware, AuthConfig};
use crate::rate_limiter::{rate_limit_middleware, RateLimiter};
use crate::session_manager::SessionManager;
use crate::types::*;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<SessionManager>,
    pub auth_config: Arc<AuthConfig>,
    pub rate_limiter: Arc<RateLimiter>,
    pub allowed_origins: Vec<String>,
}

/// Create the Axum router with all routes
pub fn create_router(state: AppState) -> Router {
    // Build CORS layer
    let cors = if state.allowed_origins.is_empty() {
        // Permissive for development
        CorsLayer::permissive()
    } else {
        CorsLayer::new()
            .allow_origin(
                state
                    .allowed_origins
                    .iter()
                    .map(|s| s.parse().unwrap())
                    .collect::<Vec<_>>(),
            )
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let auth_config = state.auth_config.clone();
    let rate_limiter = state.rate_limiter.clone();

    Router::new()
        .route("/health", get(health_check))
        .route("/api/v1/sessions", post(create_session).get(list_sessions))
        .route(
            "/api/v1/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/api/v1/sessions/{id}/messages", post(send_message))
        .route("/ws/sessions/{id}", get(ws_upgrade))
        // Rate limiter runs after auth (innermost = last in request pipeline)
        .layer(middleware::from_fn(
            move |addr: ConnectInfo<SocketAddr>, req, next| {
                let rl = rate_limiter.clone();
                async move { rate_limit_middleware(addr, rl, req, next).await }
            },
        ))
        .layer(middleware::from_fn(move |req, next| {
            auth_middleware(auth_config.clone(), req, next)
        }))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

/// Start the gateway server
pub async fn start_server(state: AppState, host: &str, port: u16) -> anyhow::Result<()> {
    let router = create_router(state);
    let addr = format!("{}:{}", host, port);

    info!(addr = %addr, "Starting gateway server");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("Gateway server stopped");
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for Ctrl+C");
    info!("Shutdown signal received, draining connections...");
    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    info!("Drain complete, shutting down");
}

// --- REST Handlers ---

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionResponse>), (StatusCode, Json<ErrorResponse>)> {
    let agent_name = req.agent_id.as_deref();
    match state.session_manager.create(agent_name).await {
        Ok(session_id) => {
            let (name, created_at, count) = state
                .session_manager
                .get_session_info(&session_id)
                .await
                .unwrap();
            Ok((
                StatusCode::CREATED,
                Json(SessionResponse {
                    session_id,
                    agent_name: name,
                    created_at,
                    message_count: count,
                }),
            ))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

async fn list_sessions(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.session_manager.list_sessions().await)
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.session_manager.get_session_info(&id).await {
        Ok((name, created_at, count)) => Ok(Json(SessionResponse {
            session_id: id,
            agent_name: name,
            created_at,
            message_count: count,
        })),
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    match state.session_manager.delete_session(&id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

const MAX_MESSAGE_LENGTH: usize = 50_000; // 50KB

async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Input validation
    if req.content.len() > MAX_MESSAGE_LENGTH {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: format!(
                    "Message content exceeds maximum length of {} bytes",
                    MAX_MESSAGE_LENGTH
                ),
            }),
        ));
    }

    match state.session_manager.send_message(&id, &req.content).await {
        Ok(content) => Ok(Json(MessageResponse {
            content,
            session_id: id,
        })),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )),
    }
}

// --- WebSocket Handler ---

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, session_id, state))
}

const WS_IDLE_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_secs(300);

async fn handle_ws_connection(socket: WebSocket, session_id: String, state: AppState) {
    use futures_util::{SinkExt, StreamExt};
    use tokio::time::timeout;

    let event_rx = match state.session_manager.subscribe(&session_id).await {
        Ok(rx) => rx,
        Err(_) => return,
    };

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut event_rx = event_rx;

    // Forward session events to WebSocket client
    let send_task = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                if ws_sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle incoming client messages with idle timeout
    let sm = state.session_manager.clone();
    let sid = session_id.clone();
    loop {
        match timeout(WS_IDLE_TIMEOUT, ws_receiver.next()).await {
            Ok(Some(Ok(msg))) => {
                if let Message::Text(text) = msg {
                    // Input validation for WebSocket messages
                    if text.len() > MAX_MESSAGE_LENGTH {
                        info!("WebSocket message exceeds max length, ignoring");
                        continue;
                    }

                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        match client_msg {
                            ClientMessage::SendMessage { content } => {
                                let sm = sm.clone();
                                let sid = sid.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = sm.send_message(&sid, &content).await {
                                        tracing::error!(error = %e, "WebSocket message processing failed");
                                    }
                                });
                            }
                            ClientMessage::Cancel => {
                                // Cancel support deferred
                            }
                        }
                    }
                }
            }
            Ok(Some(Err(_))) => {
                info!("WebSocket error, closing connection");
                break;
            }
            Ok(None) => {
                info!("WebSocket connection closed by client");
                break;
            }
            Err(_) => {
                info!(
                    "WebSocket idle timeout ({}s), closing connection",
                    WS_IDLE_TIMEOUT.as_secs()
                );
                break;
            }
        }
    }

    send_task.abort();
}
