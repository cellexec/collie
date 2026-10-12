//! collie: Mehrere Chats auf einmal. `x` markiert einen Chat, `p` schickt
//! einen Prompt an alle markierten, `O` startet einen Orchestrator für sie.
//! Ohne Markierung gelten `p` und `O` für den Chat unter dem Cursor.

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
                self.orchestrate_chats(outcome)
            }
            KeyCode::Char('o') if modifiers == KeyModifiers::SHIFT => {
                self.orchestrate_chats(outcome)
            }
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
            target: ClientRenameTarget::ChatPrompt { pane_ids },
        }));
        outcome.repaint = true;
    }

    fn orchestrate_chats(&mut self, outcome: &mut ClientShellInput) {
        let pane_ids = self.marked_chat_panes();
        if pane_ids.is_empty() {
            return;
        }
        let method = crate::api::schema::Method::ChatOrchestrate(
            crate::api::schema::ChatOrchestrateParams { pane_ids },
        );
        if !self.chat_method_supported(&method, outcome) {
            return;
        }
        self.push_endpoint_method(method, outcome);
        self.sidebar_focus.marked.clear();
        self.leave_sidebar(false, outcome);
        outcome.repaint = true;
    }
}
