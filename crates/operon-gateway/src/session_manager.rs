use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use tokio::sync::{broadcast, RwLock};

use operon_runtime::{Agent, AgentConfig, LLMProvider, Runtime};

use crate::types::SessionEvent;

/// Manages active agent sessions with broadcast support
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, AgentSession>>>,
    event_buses: Arc<RwLock<HashMap<String, broadcast::Sender<SessionEvent>>>>,
    provider: Arc<dyn LLMProvider>,
    runtime: Arc<Runtime>,
}

/// Active agent session
pub struct AgentSession {
    pub agent: Agent,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
}

impl SessionManager {
    pub fn new(provider: Arc<dyn LLMProvider>, runtime: Arc<Runtime>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_buses: Arc::new(RwLock::new(HashMap::new())),
            provider,
            runtime,
        }
    }

    /// Create a new agent session, returns session ID
    pub async fn create(&self, agent_name: Option<&str>) -> Result<String> {
        let config = AgentConfig {
            name: agent_name.unwrap_or("default").to_string(),
            ..AgentConfig::default()
        };

        let agent = Agent::new(config, self.provider.clone(), self.runtime.clone());
        let session_id = agent.session.id.clone();
        let now = Utc::now();

        let session = AgentSession {
            agent,
            created_at: now,
            last_active: now,
        };

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);

        let (tx, _) = broadcast::channel(100);
        self.event_buses
            .write()
            .await
            .insert(session_id.clone(), tx);

        Ok(session_id)
    }

    /// Send message to agent, returns response text.
    ///
    /// Uses remove/insert pattern to avoid holding write lock during LLM call.
    /// If two concurrent sends target the same session, the second gets "Session not found".
    pub async fn send_message(&self, session_id: &str, content: &str) -> Result<String> {
        // 1. Remove session from map (short write lock)
        let mut session = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| anyhow!("Session not found: {}", session_id))?
        };
        // Write lock released here

        // 2. Process message without holding any lock
        session.last_active = Utc::now();
        let response = session.agent.process_message(content).await;

        // 3. Re-insert session (short write lock) — even on error to prevent session loss
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.to_string(), session);
        }

        // 3a. Detect if session was deleted during processing (event_bus removed)
        if !self.event_buses.read().await.contains_key(session_id) {
            // Session was deleted while we were processing; remove the orphan
            self.sessions.write().await.remove(session_id);
            return Err(anyhow!("Session deleted during message processing"));
        }

        // 4. Handle result and broadcast
        let response = response?;

        if let Some(tx) = self.event_buses.read().await.get(session_id) {
            let _ = tx.send(SessionEvent::AgentResponse {
                content: response.clone(),
            });
        }

        Ok(response)
    }

    /// Get session info (non-mutable)
    pub async fn get_session_info(&self, session_id: &str) -> Result<(String, String, usize)> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| anyhow!("Session not found: {}", session_id))?;

        Ok((
            session.agent.config.name.clone(),
            session.created_at.to_rfc3339(),
            session.agent.session.message_count(),
        ))
    }

    /// List all session IDs
    pub async fn list_sessions(&self) -> Vec<String> {
        self.sessions.read().await.keys().cloned().collect()
    }

    /// Delete a session
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| anyhow!("Session not found: {}", session_id))?;
        self.event_buses.write().await.remove(session_id);
        Ok(())
    }

    /// Subscribe to session events (for WebSocket)
    pub async fn subscribe(&self, session_id: &str) -> Result<broadcast::Receiver<SessionEvent>> {
        let buses = self.event_buses.read().await;
        let tx = buses
            .get(session_id)
            .ok_or_else(|| anyhow!("Session not found: {}", session_id))?;
        Ok(tx.subscribe())
    }
}
