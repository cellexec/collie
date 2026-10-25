//! collie: `z` in der Chat-Liste versteckt einen Chat für eine Weile.

use super::*;
use crossterm::event::KeyModifiers;

/// Dauern der Snooze-Auswahl, in Anzeigereihenfolge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SnoozeChoice {
    Hour,
    FourHours,
    Morning,
    Day,
    Clear,
}

const SNOOZE_CHOICES: [SnoozeChoice; 5] = [
    SnoozeChoice::Hour,
    SnoozeChoice::FourHours,
    SnoozeChoice::Morning,
    SnoozeChoice::Day,
    SnoozeChoice::Clear,
];

const HOUR_MS: u64 = 3_600_000;
const DAY_SECS: u64 = 24 * 3_600;
/// Uhrzeit von „morgen früh" in Sekunden seit Mitternacht.
const MORNING_SECS: u64 = 8 * 3_600;

#[derive(Debug)]
pub(super) struct ClientSnoozeOverlay {
    pub(super) workspace_id: String,
    pub(super) highlighted: usize,
    /// Ob die Auswahl „heute 08:00" statt „morgen 08:00" anbietet.
    pub(super) morning_today: bool,
}

impl ClientSnoozeOverlay {
    pub(super) fn labels(&self) -> Vec<String> {
        SNOOZE_CHOICES
            .iter()
            .map(|choice| match choice {
                SnoozeChoice::Hour => " 1  1 hour".to_owned(),
                SnoozeChoice::FourHours => " 2  4 hours".to_owned(),
                SnoozeChoice::Morning if self.morning_today => " 3  today 08:00".to_owned(),
                SnoozeChoice::Morning => " 3  tomorrow 08:00".to_owned(),
                SnoozeChoice::Day => " 4  1 day".to_owned(),
                SnoozeChoice::Clear => " u  unsnooze".to_owned(),
            })
            .collect()
    }
}

/// Sekunden bis zum nächsten 08:00 Ortszeit. Vor 08:00 ist das heute.
pub(super) fn secs_until_morning(local_secs_of_day: u64) -> u64 {
    if local_secs_of_day < MORNING_SECS {
        MORNING_SECS - local_secs_of_day
    } else {
        DAY_SECS - local_secs_of_day + MORNING_SECS
    }
}

fn local_secs_of_day() -> Option<u64> {
    let local = crate::platform::local_datetime()?;
    Some(
        u64::from(local.hour()) * 3_600
            + u64::from(local.minute()) * 60
            + u64::from(local.second()),
    )
}

/// Ende eines Snooze ab `now` (Unix-Millisekunden); `None` hebt ihn auf.
fn snooze_until(choice: SnoozeChoice, now: u64, local_secs_of_day: u64) -> Option<u64> {
    match choice {
        SnoozeChoice::Hour => Some(now + HOUR_MS),
        SnoozeChoice::FourHours => Some(now + 4 * HOUR_MS),
        SnoozeChoice::Morning => Some(now + secs_until_morning(local_secs_of_day) * 1_000),
        SnoozeChoice::Day => Some(now + 24 * HOUR_MS),
        SnoozeChoice::Clear => None,
    }
}

impl ClientShellState {
    pub(super) fn open_snooze_picker(&mut self, outcome: &mut ClientShellInput) {
        let Some(cursor) = self.sidebar_focus.agent_cursor.clone() else {
            return;
        };
        let Some(row) = self.sidebar_chat_row(&cursor.pane_id) else {
            return;
        };
        let method = crate::api::schema::Method::WorkspaceSnooze(
            crate::api::schema::WorkspaceSnoozeParams {
                workspace_id: String::new(),
                until: None,
            },
        );
        if !self.supports_endpoint_method(&method) {
            outcome.repaint |= self.push_endpoint_notice(
                ClientEndpointNoticeKind::Unsupported,
                "workspace.snooze",
                "Action unavailable",
                "This server cannot snooze chats yet. Update and restart it to enable this action.",
            );
            return;
        }
        let morning_today = local_secs_of_day().is_some_and(|secs| secs < MORNING_SECS);
        self.overlay = Some(ClientShellOverlay::Snooze(ClientSnoozeOverlay {
            workspace_id: row.workspace_id,
            highlighted: 0,
            morning_today,
        }));
        outcome.repaint = true;
    }

    pub(super) fn route_snooze_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(ClientShellOverlay::Snooze(picker)) = self.overlay.as_mut() else {
            return false;
        };
        let last = SNOOZE_CHOICES.len() - 1;
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        outcome.repaint = true;
        match code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Char('p') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                picker.highlighted = (picker.highlighted + 1).min(last);
            }
            KeyCode::Char('n') if modifiers == KeyModifiers::CONTROL => {
                picker.highlighted = (picker.highlighted + 1).min(last);
            }
            KeyCode::Enter => {
                let choice = SNOOZE_CHOICES[picker.highlighted.min(last)];
                self.apply_snooze(choice, outcome);
            }
            KeyCode::Char(digit @ '1'..='4') if modifiers.is_empty() => {
                let choice = SNOOZE_CHOICES[usize::from(digit as u8 - b'1')];
                self.apply_snooze(choice, outcome);
            }
            KeyCode::Char('u') if modifiers.is_empty() => {
                self.apply_snooze(SnoozeChoice::Clear, outcome);
            }
            _ => {}
        }
        true
    }

    fn apply_snooze(&mut self, choice: SnoozeChoice, outcome: &mut ClientShellInput) {
        let Some(ClientShellOverlay::Snooze(picker)) = self.overlay.take() else {
            return;
        };
        let until = snooze_until(
            choice,
            crate::workspace::unix_millis_now(),
            local_secs_of_day().unwrap_or(0),
        );
        // Der Chat wechselt die Ansicht; der Cursor rückt auf den Nachbarn.
        let targets = self.sidebar_agent_targets();
        let next = self
            .sidebar_focus
            .agent_cursor
            .as_ref()
            .and_then(|cursor| targets.iter().position(|target| target == cursor))
            .and_then(|index| {
                targets
                    .get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|prev| targets.get(prev)))
            })
            .cloned();
        self.push_endpoint_method(
            crate::api::schema::Method::WorkspaceSnooze(
                crate::api::schema::WorkspaceSnoozeParams {
                    workspace_id: picker.workspace_id,
                    until,
                },
            ),
            outcome,
        );
        self.sidebar_focus.agent_cursor = next;
        outcome.repaint = true;
    }

    /// Neu zeichnen, wenn ein Snooze abläuft, der Spinner weiterdreht, die
    /// Restzeit springt oder eine Karte nachleuchtet. Im leeren Overlay döst
    /// die Collie.
    pub(crate) fn tick_chat_sidebar(&mut self) -> bool {
        if !self.spaces_hidden() {
            return false;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return false;
        };
        let now = crate::workspace::unix_millis_now();
        let snoozed = snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.snoozed_until.is_some_and(|until| until > now))
            .count() as u64;
        let mut key = snoozed;
        let view = self
            .sidebar_focus
            .render_view(self.mode, self.prefix_return_navigate);
        if (view.show_hidden && !self.sidebar_collapsed) || self.chat_overlay_open() {
            let spinning = snapshot
                .agents
                .iter()
                .any(|agent| agent.agent_status == crate::api::schema::AgentStatus::Working);
            if spinning {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chats::SPINNER_FRAME_MS);
            }
            if snoozed > 0 {
                key = key.wrapping_mul(31).wrapping_add(now / 60_000);
            }
        }
        if self.chat_overlay_open() {
            // Wärmestreifen und Aktivitätskurve wandern minütlich weiter.
            key = key.wrapping_mul(31).wrapping_add(now / 60_000);
            if snapshot
                .workspaces
                .iter()
                .any(|workspace| super::chat_visuals::pulsing(workspace.attention_since, now))
            {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chats::SPINNER_FRAME_MS);
            }
            if super::chats::visible_chat_rows(snapshot, false, false, view.agent_query, now)
                .is_empty()
            {
                key = key
                    .wrapping_mul(31)
                    .wrapping_add(now / super::chat_visuals::DOZE_FRAME_MS);
            }
        }
        if let Some(phase) = self.chat_pulse_phase(now) {
            key = key.wrapping_mul(31).wrapping_add(phase + 1);
        }
        let changed = self.chat_tick_key != Some(key);
        self.chat_tick_key = Some(key);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morning_is_today_before_eight_and_tomorrow_after() {
        assert_eq!(secs_until_morning(7 * 3_600), 3_600);
        assert_eq!(secs_until_morning(8 * 3_600), DAY_SECS);
        assert_eq!(secs_until_morning(22 * 3_600), 10 * 3_600);
    }

    #[test]
    fn snooze_choices_map_to_deadlines() {
        let now = 1_000_000;
        assert_eq!(
            snooze_until(SnoozeChoice::Hour, now, 0),
            Some(now + HOUR_MS)
        );
        assert_eq!(
            snooze_until(SnoozeChoice::Morning, now, 7 * 3_600),
            Some(now + HOUR_MS)
        );
        assert_eq!(snooze_until(SnoozeChoice::Clear, now, 0), None);
    }
}
