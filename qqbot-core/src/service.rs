//! Shared in-memory state for the foreground QQ bridge.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

use crate::events::NormalizedEvent;

/// Current session lifecycle state exposed by the local API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The service has started but is not yet connected.
    Booting,
    /// The service is waiting for QR-code login or QQ initialization.
    WaitingForLogin,
    /// The bridge is connected and ready to send or receive messages.
    Connected,
    /// The bridge has lost its connection to the QQ runtime.
    Disconnected,
}

/// Public session snapshot returned by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Current service lifecycle state.
    pub status: SessionStatus,
    /// Logged-in QQ identifier when available.
    pub self_id: Option<i64>,
    /// Logged-in QQ nickname when available.
    pub nickname: Option<String>,
    /// Foreground QQ process identifier when available.
    pub qq_pid: Option<u32>,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            status: SessionStatus::Booting,
            self_id: None,
            nickname: None,
            qq_pid: None,
        }
    }
}

/// Simplified friend profile exposed by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendProfile {
    /// QQ identifier.
    pub user_id: i64,
    /// QQ nickname.
    pub nickname: String,
    /// Optional remark shown in contacts.
    pub remark: Option<String>,
}

/// Simplified group profile exposed by the local API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupProfile {
    /// Group identifier.
    pub group_id: i64,
    /// Group display name.
    pub group_name: String,
}

/// Message send result returned from the bridge worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageReceipt {
    /// Message identifier returned by the underlying transport.
    pub message_id: i64,
}

/// Internal command sent from the API layer to the bridge worker.
#[derive(Debug)]
pub enum ServiceCommand {
    /// Send a private message.
    SendPrivate {
        /// Target QQ identifier.
        user_id: i64,
        /// Plain-text message content.
        text: String,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<SendMessageReceipt>>,
    },
    /// Send a group message.
    SendGroup {
        /// Target group identifier.
        group_id: i64,
        /// Plain-text message content.
        text: String,
        /// Response channel for completion.
        respond_to: oneshot::Sender<Result<SendMessageReceipt>>,
    },
}

#[derive(Debug)]
struct ServiceInner {
    session: RwLock<SessionSnapshot>,
    friends: RwLock<Vec<FriendProfile>>,
    groups: RwLock<Vec<GroupProfile>>,
    events_tx: broadcast::Sender<NormalizedEvent>,
    command_tx: mpsc::Sender<ServiceCommand>,
}

/// Cloneable handle shared across the API layer and runtime worker.
#[derive(Clone, Debug)]
pub struct ServiceState {
    inner: Arc<ServiceInner>,
}

impl ServiceState {
    /// Build a new service state around the provided command channel.
    pub fn new(command_tx: mpsc::Sender<ServiceCommand>) -> Self {
        let (events_tx, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(ServiceInner {
                session: RwLock::new(SessionSnapshot::default()),
                friends: RwLock::new(Vec::new()),
                groups: RwLock::new(Vec::new()),
                events_tx,
                command_tx,
            }),
        }
    }

    /// Build a test-friendly state with no active bridge worker.
    pub fn for_tests() -> Self {
        let (command_tx, mut command_rx) = mpsc::channel(8);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    ServiceCommand::SendPrivate {
                        user_id,
                        text,
                        respond_to,
                    } => {
                        let _ = respond_to.send(Ok(SendMessageReceipt {
                            message_id: user_id.saturating_add(text.len() as i64),
                        }));
                    },
                    ServiceCommand::SendGroup {
                        group_id,
                        text,
                        respond_to,
                    } => {
                        let _ = respond_to.send(Ok(SendMessageReceipt {
                            message_id: group_id.saturating_add(text.len() as i64),
                        }));
                    },
                }
            }
        });
        Self::new(command_tx)
    }

    /// Replace the current session snapshot.
    pub async fn set_session(&self, snapshot: SessionSnapshot) {
        *self.inner.session.write().await = snapshot;
    }

    /// Read the current session snapshot.
    pub async fn session(&self) -> SessionSnapshot {
        self.inner.session.read().await.clone()
    }

    /// Replace the cached friend list.
    pub async fn set_friends(&self, friends: Vec<FriendProfile>) {
        *self.inner.friends.write().await = friends;
    }

    /// Read the cached friend list.
    pub async fn friends(&self) -> Vec<FriendProfile> {
        self.inner.friends.read().await.clone()
    }

    /// Replace the cached group list.
    pub async fn set_groups(&self, groups: Vec<GroupProfile>) {
        *self.inner.groups.write().await = groups;
    }

    /// Read the cached group list.
    pub async fn groups(&self) -> Vec<GroupProfile> {
        self.inner.groups.read().await.clone()
    }

    /// Publish a normalized event to local websocket subscribers.
    pub fn publish_event(&self, event: NormalizedEvent) {
        let _ = self.inner.events_tx.send(event);
    }

    /// Subscribe to normalized events for websocket streaming.
    pub fn subscribe_events(&self) -> broadcast::Receiver<NormalizedEvent> {
        self.inner.events_tx.subscribe()
    }

    /// Dispatch a private-message send request to the bridge worker.
    pub async fn send_private_message(
        &self,
        user_id: i64,
        text: String,
    ) -> Result<SendMessageReceipt> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SendPrivate {
                user_id,
                text,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }

    /// Dispatch a group-message send request to the bridge worker.
    pub async fn send_group_message(
        &self,
        group_id: i64,
        text: String,
    ) -> Result<SendMessageReceipt> {
        let (respond_to, response_rx) = oneshot::channel();
        self.inner
            .command_tx
            .send(ServiceCommand::SendGroup {
                group_id,
                text,
                respond_to,
            })
            .await
            .map_err(|_| anyhow!("bridge worker is not available"))?;
        response_rx
            .await
            .map_err(|_| anyhow!("bridge worker dropped the response"))?
    }
}
