//! collie: Mehrere Chats auf einmal. `x` markiert einen Chat, `p` schickt
//! einen Prompt an alle markierten. Ohne Markierung gilt `p` für den Chat
//! unter dem Cursor.
//!
//! Master Chats (docs/specs/master-chat.md): `O` macht den Chat unter dem
//! Cursor zum Master oder hängt die markierten Chats an einen Master, `M`
//! startet einen neuen Master, `m` öffnet das Telescope eines Masters.

use super::sidebar_focus::SidebarStep;
use super::*;
use crossterm::event::KeyModifiers;

/// Vorbelegung der Prompt-Eingabe: bringt Chats zu einem Abschluss.
pub(super) const WRAP_UP_PROMPT: &str = "Fasse deinen Stand zusammen: Was ist erledigt, was ist offen, und was fehlt noch, damit dieser Chat geschlossen werden kann? Arbeite auf diesen Abschluss hin.";

impl ClientShellState {
    /// Liefert, ob die Taste zu den Markierungen gehörte.
    pub(super) fn route_chat_marks_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let plain = modifiers.is_empty();
        match code {
            KeyCode::Char('x') if plain => self.toggle_chat_mark(outcome),
            KeyCode::Char('p') if plain => self.open_chat_prompt(outcome),
            // Mit Kitty-Tastaturprotokoll kommt `O` als `o` mit Shift.
            KeyCode::Char('O') if plain || modifiers == KeyModifiers::SHIFT => {
                self.make_master(outcome)
            }
            KeyCode::Char('o') if modifiers == KeyModifiers::SHIFT => self.make_master(outcome),
            KeyCode::Char('M') if plain || modifiers == KeyModifiers::SHIFT => {
                self.create_master(outcome)
            }
            KeyCode::Char('m') if modifiers == KeyModifiers::SHIFT => self.create_master(outcome),
            // Mit Kitty-Tastaturprotokoll kommt `I` als `i` mit Shift.
            KeyCode::Char('I') if plain || modifiers == KeyModifiers::SHIFT => {
                return self.open_cursor_user_inbox(outcome);
            }
            KeyCode::Char('i') if modifiers == KeyModifiers::SHIFT => {
                return self.open_cursor_user_inbox(outcome);
            }
            // `m` ohne Master unter dem Cursor bleibt das globale Menü.
            KeyCode::Char('m') if plain => return self.open_cursor_telescope(outcome),
            _ => return false,
        }
        true
    }

    /// Markiert den Chat unter dem Cursor oder hebt die Markierung auf und
    /// rückt den Cursor weiter.
    fn toggle_chat_mark(&mut self, outcome: &mut ClientShellInput) {
        let Some(cursor) = self.local_sidebar_agent_cursor() else {
            return;
        };
        let marked = &mut self.sidebar_focus.marked;
        if let Some(index) = marked.iter().position(|mark| *mark == cursor) {
            marked.remove(index);
        } else {
            marked.push(cursor);
        }
        self.step_sidebar(SidebarStep::Delta(1), outcome);
        outcome.repaint = true;
    }

    /// Panes der markierten Chats, die es noch gibt; ohne Markierung der
    /// Chat unter dem Cursor.
    pub(super) fn marked_chat_panes(&self) -> Vec<String> {
        let marked = self
            .sidebar_focus
            .marked
            .iter()
            .filter(|mark| mark.endpoint_id == self.active_endpoint_id)
            .filter(|mark| self.sidebar_chat_row(&mark.pane_id).is_some())
            .map(|mark| mark.pane_id.clone())
            .collect::<Vec<_>>();
        if !marked.is_empty() {
            return marked;
        }
        self.local_sidebar_agent_cursor()
            .map(|cursor| cursor.pane_id)
            .into_iter()
            .collect()
    }

    fn chat_method_supported(
        &mut self,
        method: &crate::api::schema::Method,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if self.supports_endpoint_method(method) {
            return true;
        }
        outcome.repaint |= self.push_endpoint_notice(
            ClientEndpointNoticeKind::Unsupported,
            crate::api::api_method_name(method),
            "Action unavailable",
            "This server cannot do this for several chats yet. Update and restart it to enable this action.",
        );
        false
    }

    fn open_chat_prompt(&mut self, outcome: &mut ClientShellInput) {
        let pane_ids = self.marked_chat_panes();
        if pane_ids.is_empty() {
            return;
        }
        let method = crate::api::schema::Method::ChatPrompt(crate::api::schema::ChatPromptParams {
            pane_ids: Vec::new(),
            text: String::new(),
        });
        if !self.chat_method_supported(&method, outcome) {
            return;
        }
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: if pane_ids.len() == 1 {
                "prompt chat"
            } else {
                "prompt marked chats"
            },
            input: TextEditor::new(WRAP_UP_PROMPT, false),
            target: ClientRenameTarget::ChatPrompt {
                pane_ids,
                report_workspace_id: None,
            },
        }));
        outcome.repaint = true;
    }

    /// `O`: ohne Markierung wird der Chat unter dem Cursor zum Master. Mit
    /// Markierung werden die markierten Chats Children des Masters unter dem
    /// Cursor oder eines neuen Masters.
    fn make_master(&mut self, outcome: &mut ClientShellInput) {
        use crate::api::schema::{ChatAdoptParams, ChatMasterCreateParams, ChatWorkspaceParams};
        let Some(cursor) = self
            .local_sidebar_agent_cursor()
            .and_then(|cursor| self.sidebar_chat_row(&cursor.pane_id))
        else {
            return;
        };
        let marked = self
            .sidebar_focus
            .marked
            .iter()
            .filter(|mark| mark.endpoint_id == self.active_endpoint_id)
            .filter_map(|mark| self.sidebar_chat_row(&mark.pane_id))
            .filter(|row| !row.master && row.workspace_id != cursor.workspace_id)
            .map(|row| row.workspace_id)
            .collect::<Vec<_>>();
        let method = if marked.is_empty() {
            crate::api::schema::Method::ChatPromote(ChatWorkspaceParams {
                workspace_id: Some(cursor.workspace_id),
                caller_pane_id: None,
            })
        } else if cursor.master {
            crate::api::schema::Method::ChatAdopt(ChatAdoptParams {
                master_workspace_id: cursor.workspace_id,
                workspace_ids: marked,
            })
        } else {
            crate::api::schema::Method::ChatMasterCreate(ChatMasterCreateParams {
                cwd: None,
                focus: true,
                adopt_workspace_ids: marked,
            })
        };
        if !self.chat_method_supported(&method, outcome) {
            return;
        }
        self.push_endpoint_method(method, outcome);
        self.sidebar_focus.marked.clear();
        outcome.repaint = true;
    }

    /// `M`: neuer Master Chat im Home-Verzeichnis.
    fn create_master(&mut self, outcome: &mut ClientShellInput) {
        let method = crate::api::schema::Method::ChatMasterCreate(
            crate::api::schema::ChatMasterCreateParams {
                cwd: None,
                focus: true,
                adopt_workspace_ids: Vec::new(),
            },
        );
        if !self.chat_method_supported(&method, outcome) {
            return;
        }
        self.push_endpoint_method(method, outcome);
        self.leave_sidebar(false, outcome);
        outcome.repaint = true;
    }

    /// `I` in der Sidebar öffnet die globale Nutzer-Inbox.
    fn open_cursor_user_inbox(&mut self, outcome: &mut ClientShellInput) -> bool {
        self.open_user_inbox(outcome);
        true
    }

    /// `m` auf einer Master-Karte öffnet sein Telescope.
    fn open_cursor_telescope(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(row) = self
            .local_sidebar_agent_cursor()
            .and_then(|cursor| self.sidebar_chat_row(&cursor.pane_id))
        else {
            return false;
        };
        let (master, select) = match (row.master, row.parent) {
            (true, _) => (row.workspace_id, None),
            (false, Some(parent)) => (parent, Some(row.workspace_id)),
            (false, None) => return false,
        };
        self.open_chat_telescope(master, select, outcome);
        true
    }
}
