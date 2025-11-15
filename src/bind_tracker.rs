use std::fmt;

use tokio::sync::RwLock;

#[derive(Clone, Debug)]
pub enum BindState {
    Pending,
    Connecting,
    Bound,
    Error(String),
}

impl fmt::Display for BindState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindState::Pending => write!(f, "pending"),
            BindState::Connecting => write!(f, "connecting"),
            BindState::Bound => write!(f, "bound"),
            BindState::Error(err) => write!(f, "error: {err}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BindStatus {
    pub state: BindState,
    pub last_message_id: Option<String>,
}

impl BindStatus {
    fn new(state: BindState) -> Self {
        Self {
            state,
            last_message_id: None,
        }
    }
}

pub struct BindTracker {
    statuses: RwLock<Vec<BindStatus>>,
}

impl BindTracker {
    pub fn new(size: usize) -> Self {
        let statuses = (0..size)
            .map(|_| BindStatus::new(BindState::Pending))
            .collect();

        Self {
            statuses: RwLock::new(statuses),
        }
    }

    pub async fn set_state(&self, idx: usize, state: BindState) {
        if let Some(entry) = self.statuses.write().await.get_mut(idx) {
            entry.state = state;
        }
    }

    pub async fn set_last_message_id(&self, idx: usize, message_id: Option<String>) {
        if let Some(entry) = self.statuses.write().await.get_mut(idx) {
            entry.last_message_id = message_id;
        }
    }

    pub async fn snapshot(&self) -> Vec<BindStatus> {
        self.statuses.read().await.clone()
    }
}
