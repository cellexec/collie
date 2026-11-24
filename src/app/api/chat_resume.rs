//! collie: Prompts an einen Chat, den `chat.resume` gerade fortsetzt.
//!
//! Nach `chat.resume` tippt herdr `claude --resume <id>` in die Shell des
//! Panes. Claude meldet sich schnell als idle, lädt dann aber noch die alte
//! Session und verwirft dabei Eingaben. Ein Prompt in dieser Zeit ging
//! verloren. collie hält Prompts deshalb zurück, bis Claude bereit ist:
//!
//! - Claude hat die fortgesetzte Session per Hook gemeldet,
//! - der Agent läuft und ist idle,
//! - der untere Bildschirm war [`RESUME_QUIET_MS`] lang unverändert.
//!
//! `agent.prompt` antwortet erst, wenn der Prompt wirklich zugestellt ist.
//! Nach [`RESUME_SETTLE_MAX_MS`] oder einem `chat.stop` gibt collie das Warten
//! auf und stellt zu, was zurückgehalten wurde.

use std::hash::{Hash, Hasher};

use crate::app::App;
use crate::workspace::ResumeSettle;

/// So lange muss der untere Bildschirm nach dem Start ruhig sein.
pub(crate) const RESUME_QUIET_MS: u64 = 2_000;
/// Spätestens dann stellt collie zurückgehaltene Prompts trotzdem zu.
pub(crate) const RESUME_SETTLE_MAX_MS: u64 = 60_000;

/// Eine `agent.prompt`-Anfrage, die auf einen fortgesetzten Chat wartet.
pub(crate) struct HeldAgentPrompt {
    workspace_id: String,
    request: crate::api::schema::Request,
    respond_to: std::sync::mpsc::Sender<String>,
}

impl App {
    /// Merkt sich, dass ein Chat gerade fortgesetzt wird.
    pub(super) fn begin_resume_settle(&mut self, index: usize, millis: u64) {
        if let Some(ws) = self.state.workspaces.get_mut(index) {
            ws.chat.resume_settle = Some(ResumeSettle {
                resumed_at: millis,
                session_started: false,
                screen: 0,
                quiet_since: millis,
            });
        }
    }

    /// Der Hook hat die Session eines Panes gemeldet. Gehört das Pane zu einem
    /// Chat, der gerade fortgesetzt wird, hat Claude seine Session geladen.
    pub(super) fn note_resume_session_started(&mut self, index: usize) {
        if let Some(settle) = self
            .state
            .workspaces
            .get_mut(index)
            .and_then(|ws| ws.chat.resume_settle.as_mut())
        {
            settle.session_started = true;
        }
    }

    /// Der Chat wird fortgesetzt und nimmt noch keine Prompts an.
    pub(super) fn chat_resume_settling(&self, index: usize) -> bool {
        self.state
            .workspaces
            .get(index)
            .is_some_and(|ws| ws.chat.resume_settle.is_some())
    }

    /// Hält eine `agent.prompt`-Anfrage zurück, wenn ihr Ziel ein Chat ist,
    /// der gerade fortgesetzt wird. Liefert die Anfrage sonst zurück.
    pub(super) fn hold_agent_prompt_while_resuming(
        &mut self,
        request: crate::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> Option<(crate::api::schema::Request, std::sync::mpsc::Sender<String>)> {
        let crate::api::schema::Method::AgentPrompt(params) = &request.method else {
            return Some((request, respond_to));
        };
        // Gleich nach dem Resume kennt das Pane den Agent noch nicht. Ein
        // Pane-Ziel wie `w4E:p1` gehört dann trotzdem zum fortgesetzten Chat.
        let Some(index) = self
            .resolve_agent_target(&params.target)
            .ok()
            .map(|target| target.ws_idx)
            .or_else(|| {
                self.parse_current_public_pane_id(&params.target)
                    .map(|(index, _)| index)
            })
            .filter(|&index| self.chat_resume_settling(index))
        else {
            return Some((request, respond_to));
        };
        tracing::info!(target = %params.target, "agent prompt held until the resumed chat is ready");
        self.held_agent_prompts.push(HeldAgentPrompt {
            workspace_id: self.state.workspaces[index].id.clone(),
            request,
            respond_to,
        });
        self.chat_inbox_deadline = Some(std::time::Instant::now());
        None
    }

    /// Prüft fortgesetzte Chats und stellt zurückgehaltene Prompts zu, sobald
    /// ihr Chat bereit ist. Liefert (geändert, wartet noch).
    pub(super) fn advance_resume_settles(&mut self, millis: u64) -> (bool, bool) {
        let mut waiting = false;
        for index in 0..self.state.workspaces.len() {
            let Some(settle) = self.state.workspaces[index].chat.resume_settle else {
                continue;
            };
            let screen = self.chat_screen_digest(index);
            let next = next_resume_settle(settle, screen, millis, self.chat_agent_idle(index));
            self.state.workspaces[index].chat.resume_settle = next;
            waiting |= next.is_some();
        }
        let changed = self.release_held_agent_prompts();
        (changed, waiting)
    }

    /// Stellt alle zurückgehaltenen Prompts zu, deren Chat nicht mehr
    /// fortgesetzt wird. Ist der Chat weg, beantwortet `agent.prompt` das.
    fn release_held_agent_prompts(&mut self) -> bool {
        let (ready, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held_agent_prompts)
            .into_iter()
            .partition(|held| {
                self.parse_workspace_id(&held.workspace_id)
                    .is_none_or(|index| !self.chat_resume_settling(index))
            });
        self.held_agent_prompts = held;
        let mut changed = false;
        for held in ready {
            changed |= self.handle_deferred_agent_api_request(held.request, held.respond_to);
        }
        changed
    }

    /// Im Agent-Pane läuft ein Terminal, und der Agent ist idle.
    fn chat_agent_idle(&self, index: usize) -> bool {
        self.chat_agent_pane(index)
            .and_then(|pane_id| self.lookup_runtime_sender(index, pane_id))
            .is_some()
            && matches!(
                self.workspace_info(index).agent_status,
                crate::api::schema::AgentStatus::Idle | crate::api::schema::AgentStatus::Done
            )
    }

    /// Prüfsumme des unteren Bildschirms im Agent-Pane eines Chats.
    pub(super) fn chat_screen_digest(&self, index: usize) -> u64 {
        let Some(text) = self
            .chat_agent_pane(index)
            .and_then(|pane_id| self.lookup_runtime_sender(index, pane_id))
            .map(|runtime| runtime.detection_text())
        else {
            return 0;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

/// Nächster Stand eines fortgesetzten Chats. `None` heißt: Er nimmt Prompts an.
fn next_resume_settle(
    settle: ResumeSettle,
    screen: u64,
    millis: u64,
    agent_idle: bool,
) -> Option<ResumeSettle> {
    // Gleich nach `chat.resume` gilt der Chat noch als gestoppt: Der Plan steht
    // noch am Terminal, und das Pane startet erst einen Moment später. Das
    // beendet die Haltephase nicht, das tut nur `chat.stop`.
    if millis.saturating_sub(settle.resumed_at) >= RESUME_SETTLE_MAX_MS {
        return None;
    }
    if screen != settle.screen {
        return Some(ResumeSettle {
            screen,
            quiet_since: millis,
            ..settle
        });
    }
    let quiet = millis.saturating_sub(settle.quiet_since) >= RESUME_QUIET_MS;
    if settle.session_started && agent_idle && quiet {
        return None;
    }
    Some(settle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle() -> ResumeSettle {
        ResumeSettle {
            resumed_at: 1_000,
            session_started: true,
            screen: 7,
            quiet_since: 1_000,
        }
    }

    #[test]
    fn a_resumed_chat_is_ready_only_after_session_idle_and_a_quiet_screen() {
        let ready_at = 1_000 + RESUME_QUIET_MS;
        // Idle, ruhig und Session gemeldet: bereit.
        assert_eq!(next_resume_settle(settle(), 7, ready_at, true), None);
        // Der Bildschirm ist noch nicht lange genug ruhig.
        assert!(next_resume_settle(settle(), 7, ready_at - 1, true).is_some());
        // Claude meldet idle, hat seine Session aber noch nicht geladen.
        let unloaded = ResumeSettle {
            session_started: false,
            ..settle()
        };
        assert!(next_resume_settle(unloaded, 7, ready_at, true).is_some());
        // Nicht idle.
        assert!(next_resume_settle(settle(), 7, ready_at, false).is_some());
        // Der Bildschirm hat sich geändert: Die Ruhezeit beginnt neu.
        let changed = next_resume_settle(settle(), 8, ready_at, true).unwrap();
        assert_eq!((changed.screen, changed.quiet_since), (8, ready_at));
    }

    #[test]
    fn a_long_resuming_chat_stops_holding_prompts() {
        let late = 1_000 + RESUME_SETTLE_MAX_MS;
        assert!(next_resume_settle(settle(), 8, late - 1, false).is_some());
        assert_eq!(next_resume_settle(settle(), 8, late, false), None);
    }
}
