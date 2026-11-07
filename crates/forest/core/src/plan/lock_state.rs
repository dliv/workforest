//! Independently observed Git worktree lock metadata for read-only reporting.
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeLockState {
    Locked { reason: Option<String> },
    Unlocked,
    Unavailable,
}

impl WorktreeLockState {
    pub fn human_annotation(&self) -> Option<String> {
        match self {
            Self::Locked { reason: None } => Some("locked".to_string()),
            Self::Locked {
                reason: Some(reason),
            } => {
                let escaped: String = reason
                    .chars()
                    .flat_map(|c| {
                        if c.is_control() {
                            c.escape_default().collect::<Vec<_>>()
                        } else {
                            vec![c]
                        }
                    })
                    .collect();
                Some(format!("locked: {}", escaped))
            }
            Self::Unlocked => None,
            Self::Unavailable => Some("warning: lock state unavailable".to_string()),
        }
    }
}
