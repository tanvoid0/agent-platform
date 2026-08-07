//! The contract enums — **hand-maintained, and this is now the source of truth.**
//!
//! This file was generated from `app/shared_enums.py` by
//! `scripts/sync_contract_enums.py`, because the same vocabulary existed in a
//! Python server and a Rust client and the two had to agree. The Python server
//! is gone; there is one language left, and a generator that reads a file that
//! no longer exists is worse than no generator. Both are deleted.
//!
//! The values here are the wire contract. The server writes them as strings
//! (`desktop/crates/server/src/`, mostly straight into SQL), so changing a
//! variant here does not change what the server emits — grep for the string
//! before touching one. Every enum keeps a `#[serde(other)] Unknown` arm so a
//! server that learns a new status does not break an older client.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProcessStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "planning")]
    Planning,
    #[serde(rename = "approval_required")]
    ApprovalRequired,
    #[serde(rename = "approved")]
    Approved,
    #[serde(rename = "task_review_required")]
    TaskReviewRequired,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(other)]
    Unknown,
}

impl ProcessStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Planning => "planning",
            Self::ApprovalRequired => "approval_required",
            Self::Approved => "approved",
            Self::TaskReviewRequired => "task_review_required",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

pub const PROCESS_STATUSES: [ProcessStatus; 9] = [ProcessStatus::Pending, ProcessStatus::Planning, ProcessStatus::ApprovalRequired, ProcessStatus::Approved, ProcessStatus::TaskReviewRequired, ProcessStatus::Running, ProcessStatus::Completed, ProcessStatus::Failed, ProcessStatus::Cancelled];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TaskStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "awaiting_review")]
    AwaitingReview,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(other)]
    Unknown,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::AwaitingReview => "awaiting_review",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

pub const TASK_STATUSES: [TaskStatus; 5] = [TaskStatus::Pending, TaskStatus::Running, TaskStatus::AwaitingReview, TaskStatus::Completed, TaskStatus::Failed];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ReviewDecision {
    #[serde(rename = "approve")]
    Approve,
    #[serde(rename = "reject")]
    Reject,
    #[serde(rename = "request_changes")]
    RequestChanges,
    #[serde(other)]
    Unknown,
}

impl ReviewDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::RequestChanges => "request_changes",
            Self::Unknown => "unknown",
        }
    }
}

pub const REVIEW_DECISIONS: [ReviewDecision; 3] = [ReviewDecision::Approve, ReviewDecision::Reject, ReviewDecision::RequestChanges];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProcessRetryMode {
    #[serde(rename = "planning")]
    Planning,
    #[serde(rename = "execution")]
    Execution,
    #[serde(rename = "task")]
    Task,
    #[serde(other)]
    Unknown,
}

impl ProcessRetryMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Execution => "execution",
            Self::Task => "task",
            Self::Unknown => "unknown",
        }
    }
}

pub const PROCESS_RETRY_MODES: [ProcessRetryMode; 3] = [ProcessRetryMode::Planning, ProcessRetryMode::Execution, ProcessRetryMode::Task];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProcessSyncAction {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "aligned_status")]
    AlignedStatus,
    #[serde(rename = "requeued_plan")]
    RequeuedPlan,
    #[serde(rename = "requeued_execution")]
    RequeuedExecution,
    #[serde(other)]
    Unknown,
}

impl ProcessSyncAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blocked => "blocked",
            Self::AlignedStatus => "aligned_status",
            Self::RequeuedPlan => "requeued_plan",
            Self::RequeuedExecution => "requeued_execution",
            Self::Unknown => "unknown",
        }
    }
}

pub const PROCESS_SYNC_ACTIONS: [ProcessSyncAction; 5] = [ProcessSyncAction::None, ProcessSyncAction::Blocked, ProcessSyncAction::AlignedStatus, ProcessSyncAction::RequeuedPlan, ProcessSyncAction::RequeuedExecution];
