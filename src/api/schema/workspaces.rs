use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::common::AgentStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceCreateParams {
    /// Workspace whose focused pane supplies the `follow` cwd policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub focus: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceCloseParams {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub close_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceRenameParams {
    pub workspace_id: String,
    pub label: String,
}

/// collie: Submits the same prompt to the agents in several panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatPromptParams {
    pub pane_ids: Vec<String>,
    pub text: String,
}

/// collie: Role of a workspace in the master chat model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatTeamRole {
    Master,
    Child,
    #[serde(other)]
    Unknown,
}

/// collie: Master chat facts of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatTeamInfo {
    pub role: ChatTeamRole,
    /// Master of a child workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_workspace_id: Option<String>,
    /// Task the master gave this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<String>,
    /// Children of a master.
    #[serde(default)]
    pub child_count: usize,
    /// Inbox entries of a master that are not delivered yet.
    #[serde(default)]
    pub inbox_pending: usize,
    /// Latest inbox entry of a master, formatted as one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_latest: Option<String>,
}

/// collie: Optional workspace target; the caller's workspace when omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatWorkspaceParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Pane of the caller, from `HERDR_PANE_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
}

/// collie: Creates a new master chat.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatMasterCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub focus: bool,
    /// Workspaces that become children of the new master.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adopt_workspace_ids: Vec<String>,
}

/// collie: Attaches workspaces as children of a master.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatAdoptParams {
    pub master_workspace_id: String,
    pub workspace_ids: Vec<String>,
}

/// collie: Starts a child chat for a master.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatSpawnParams {
    /// Master workspace; the caller's workspace when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
    /// Existing, freshly created workspace to use, for example a worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Only check whether the master may start another child.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub check_only: bool,
}

/// collie: What happens to the children when a master closes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChatCloseChildren {
    #[default]
    Close,
    Release,
}

/// collie: Closes a chat; a master takes its children along or releases them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatCloseParams {
    pub workspace_id: String,
    #[serde(default)]
    pub children: ChatCloseChildren,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
}

/// collie: Message to the inbox of a master.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatReportParams {
    /// Child workspace the message is about; the caller's workspace when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
    pub text: String,
    /// The user wrote to the child directly.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub from_user: bool,
}

/// collie: Inbox of a master.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatInboxParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller_pane_id: Option<String>,
    /// Include delivered entries.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub all: bool,
}

/// collie: One inbox entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatInboxEntryInfo {
    pub at: u64,
    /// Child workspace, empty for entries without a child.
    pub workspace_id: String,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceSnoozeParams {
    pub workspace_id: String,
    /// Unix milliseconds until which the workspace stays snoozed; `null` clears it.
    #[serde(default)]
    pub until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceMoveParams {
    pub workspace_id: String,
    pub insert_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceMoveBlockParams {
    pub workspace_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceReportMetadataParams {
    pub workspace_id: String,
    pub source: String,
    #[schemars(schema_with = "super::common::metadata_token_patch_schema")]
    pub tokens: HashMap<String, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 86_400_000))]
    pub ttl_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    pub number: usize,
    pub label: String,
    pub focused: bool,
    pub pane_count: usize,
    pub tab_count: usize,
    pub active_tab_id: String,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[schemars(schema_with = "super::common::metadata_token_values_schema")]
    pub tokens: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorkspaceWorktreeInfo>,
    /// Since when the workspace waits for the user, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_since: Option<u64>,
    /// Until when the workspace is snoozed, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
    /// Since when the agent in the first tab is working, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_since: Option<u64>,
    /// Whether the user has opened the workspace since `attention_since`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_seen: Option<bool>,
    /// Last line of the agent's latest reply, as reported by a hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
    /// Short reason why the agent is blocked, as reported by a hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// What the agent waits for, for example `Formular: 7 Fragen`, as reported by a hook.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<String>,
    /// Agent in the first tab whose saved resume command has not started yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_agent: Option<String>,
    /// Tokens in the context window of the agent in the first tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Size of that context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Finished working spans of the agent in the first tab from the last 30
    /// minutes, as `[start, end]` in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub working_spans: Vec<(u64, u64)>,
    /// Master chat role, parent, assignment and inbox summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<ChatTeamInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceWorktreeInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}
