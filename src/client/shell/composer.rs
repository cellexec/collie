//! collie: Composer für neue Nachrichten an einen Chat, siehe
//! docs/specs/user-composer.md.
//!
//! Statt im Eingabefeld des Agents zu tippen, in das collie auch Prompts der
//! Children schreibt, öffnet `a` (Chat-Liste), `prefix+a` oder der Button
//! `Neue Nachricht` in der Tab-Leiste nvim in einem Popup. Der Server reiht die
//! Nachricht ein und stellt sie zu, sobald der Chat idle ist.

use super::*;
use crate::api::schema::{ChatUserInboxActParams, ChatUserInboxAction, Method};

/// Beschriftung des Buttons rechts in der Tab-Leiste.
pub(super) const COMPOSER_BUTTON: &str = " 󰏫 Neue Nachricht ";

/// Chat im fokussierten Space, an den der Button schreibt: nur Chats mit
/// laufendem Agent.
pub(super) fn composer_target(snapshot: &ClientShellSnapshot) -> Option<String> {
    let focused = snapshot.focused_workspace_id.as_deref()?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == focused)?;
    let row = super::chats::chat_row(snapshot, workspace)?;
    (row.has_agent && !row.stopped).then_some(row.workspace_id)
}

impl ClientShellState {
    /// Öffnet den Composer für einen Chat. Die Chat-Liste schließt, damit
    /// die Tastatur beim Popup landet.
    pub(super) fn compose_chat_message(
        &mut self,
        workspace_id: String,
        outcome: &mut ClientShellInput,
    ) {
        let method = Method::ChatUserInboxAct(ChatUserInboxActParams {
            workspace_id: Some(workspace_id),
            caller_pane_id: None,
            action: ChatUserInboxAction::Compose,
            ids: Vec::new(),
            exit_code: None,
            path: None,
            all: false,
        });
        if !self.supports_endpoint_method(&method) {
            outcome.repaint |= self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                crate::api::api_method_name(&method),
                "Action unavailable",
                "This server cannot open the message composer yet. Update and restart it.",
            );
            return;
        }
        if self.mode == ClientShellMode::Navigate {
            self.leave_sidebar(false, outcome);
        }
        self.push_endpoint_method(method, outcome);
        outcome.repaint = true;
    }

    /// `a` in der Chat-Liste: Nachricht an den Chat unter dem Cursor.
    pub(super) fn compose_cursor_chat_message(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(row) = self
            .local_sidebar_agent_cursor()
            .and_then(|cursor| self.sidebar_chat_row(&cursor.pane_id))
        else {
            return false;
        };
        self.compose_chat_message(row.workspace_id, outcome);
        true
    }

    /// `prefix+a`: Nachricht an den fokussierten Chat, sonst an den zuletzt
    /// besuchten oder ersten Master.
    pub(super) fn compose_focused_chat_message(&mut self, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let target = composer_target(snapshot).or_else(|| {
            let rows = super::chats::all_chat_rows(snapshot);
            self.last_master_workspace
                .clone()
                .filter(|last| {
                    rows.iter()
                        .any(|row| row.master && &row.workspace_id == last)
                })
                .or_else(|| {
                    rows.iter()
                        .find(|row| row.master)
                        .map(|row| row.workspace_id.clone())
                })
        });
        if let Some(target) = target {
            self.compose_chat_message(target, outcome);
        }
    }
}
