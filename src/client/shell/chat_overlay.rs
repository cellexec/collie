//! collie: Chat-Liste als Overlay (`ui.sidebar.overlay`).
//!
//! Eingeklappt bleibt links eine Linie von 1 Spalte, deren Farbe zeigt, ob
//! ein Chat wartet. Ausgefahren liegt die Chat-Liste über den Panes. Das
//! Pane-Layout ändert sich dabei nicht, kein PTY wird resized. Alles hier ist
//! TUI-Zustand; der Server sieht nur gewöhnliche Fokus-Anfragen.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect, style::Style};

use super::*;
use crate::api::schema::AgentStatus;

/// Overlay-Breite in Prozent der Terminalbreite.
pub(super) const DEFAULT_PERCENT: u16 = 45;
const MIN_PERCENT: u16 = 30;
const MAX_PERCENT: u16 = 70;
const PERCENT_STEP: u16 = 5;
const MIN_WIDTH: u16 = 50;
const MAX_WIDTH: u16 = 100;
/// So lange muss die Maus auf der Linie stehen, bis das Overlay ausfährt.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// Ein Puls ist dreimal hell und normal, je 250 ms.
const PULSE_PHASE_MS: u64 = 250;
const PULSE_MS: u64 = 6 * PULSE_PHASE_MS;

pub(super) fn clamp_percent(percent: u16) -> u16 {
    percent.clamp(MIN_PERCENT, MAX_PERCENT)
}

/// Fläche des Overlays: volle Höhe, linksbündig.
pub(super) fn overlay_rect(cols: u16, rows: u16, percent: u16) -> Rect {
    let width = (u32::from(cols) * u32::from(percent) / 100) as u16;
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH).min(cols);
    Rect::new(0, 0, width, rows)
}

/// Ob die Linie gerade hell gezeichnet wird.
pub(super) fn pulse_bright(started: Option<u64>, now: u64) -> bool {
    started.is_some_and(|started| {
        let elapsed = now.saturating_sub(started);
        elapsed < PULSE_MS && (elapsed / PULSE_PHASE_MS).is_multiple_of(2)
    })
}

/// Farbe der Linie: rot bei einem blockierten Chat, Akzent bei einem
/// wartenden, sonst gedimmt.
fn line_color(
    snapshot: &ClientShellSnapshot,
    now: u64,
    palette: &Palette,
) -> ratatui::style::Color {
    let rows = super::chats::visible_chat_rows(snapshot, false, false, "", now);
    if rows.iter().any(super::chats::ChatRow::needs_reply) {
        status_color(AgentStatus::Blocked, palette)
    } else if rows
        .iter()
        .any(|row| matches!(row.status, AgentStatus::Done | AgentStatus::Idle))
    {
        palette.accent
    } else {
        palette.surface1
    }
}

/// Zeichnet die eingeklappte Linie und meldet sie als Treffer.
pub(super) fn render_chat_line(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    bright: bool,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let color = if bright {
        palette.text
    } else {
        line_color(snapshot, crate::workspace::unix_millis_now(), palette)
    };
    let style = Style::default().fg(color).bg(palette.sidebar_bg);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol("▎").set_style(style);
            }
        }
    }
    hits.sidebar_line = area;
}

/// Jüngste Wartezeit unter den Chats mit Agent, die auf den Nutzer warten.
fn latest_attention(snapshot: &ClientShellSnapshot, now: u64) -> Option<u64> {
    super::chats::visible_chat_rows(snapshot, false, false, "", now)
        .into_iter()
        .filter(|row| {
            row.has_agent
                && matches!(
                    row.status,
                    AgentStatus::Blocked | AgentStatus::Done | AgentStatus::Idle
                )
        })
        .map(|row| row.attention_since)
        .max()
}

impl ClientShellState {
    /// Die Chat-Liste läuft als Overlay mit Linie.
    pub(super) fn chat_overlay_enabled(&self) -> bool {
        self.config.sidebar_overlay && self.spaces_hidden()
    }

    /// Das Overlay ist ausgefahren.
    pub(super) fn chat_overlay_open(&self) -> bool {
        self.chat_overlay_enabled()
            && !self.mobile_layout_active()
            && self
                .sidebar_focus
                .render_view(self.mode, self.prefix_return_navigate)
                .focused
    }

    /// Fährt das Overlay aus wie `prefix+w`. `hover` merkt sich, dass die Maus
    /// es geöffnet hat.
    pub(super) fn open_chat_overlay(&mut self, hover: bool, outcome: &mut ClientShellInput) {
        self.chat_line_hover_deadline = None;
        if self.mode != ClientShellMode::Terminal || self.overlay.is_some() {
            return;
        }
        self.pending_workspace_highlight = None;
        self.mode = ClientShellMode::Navigate;
        self.navigate_workspace_id = self.focused_navigation_target();
        self.reveal_navigation_workspace = true;
        self.enter_sidebar_focus(outcome);
        self.sidebar_focus.hover_opened = hover;
        outcome.repaint = true;
    }

    /// `<`/`>` ändern die Overlay-Breite.
    pub(super) fn adjust_chat_overlay_width(
        &mut self,
        wider: bool,
        outcome: &mut ClientShellInput,
    ) {
        let percent = clamp_percent(if wider {
            self.chat_overlay_percent.saturating_add(PERCENT_STEP)
        } else {
            self.chat_overlay_percent.saturating_sub(PERCENT_STEP)
        });
        if percent != self.chat_overlay_percent {
            self.chat_overlay_percent = percent;
            self.persist_chrome_preferences(outcome);
            outcome.repaint = true;
        }
    }

    /// Maus bei aktivem Overlay-Modus. Gibt `true` zurück, wenn das Ereignis
    /// verbraucht ist.
    pub(super) fn route_chat_overlay_mouse(
        &mut self,
        mouse: MouseEvent,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !self.chat_overlay_enabled() || self.overlay.is_some() || self.mobile_layout_active() {
            self.chat_line_hover_deadline = None;
            return false;
        }
        let point = (mouse.column, mouse.row);
        if self.chat_overlay_open() {
            return self.route_open_chat_overlay_mouse(mouse, point, outcome);
        }
        let on_line = super::contains(self.hits.sidebar_line, point);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_line => {
                self.open_chat_overlay(false, outcome);
                true
            }
            MouseEventKind::Moved if on_line => {
                if self.chat_line_hover_deadline.is_none() {
                    self.chat_line_hover_deadline = Some(std::time::Instant::now() + HOVER_DELAY);
                }
                true
            }
            MouseEventKind::Moved => {
                self.chat_line_hover_deadline = None;
                false
            }
            _ => false,
        }
    }

    fn route_open_chat_overlay_mouse(
        &mut self,
        mouse: MouseEvent,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        use super::sidebar_focus::{SidebarAgentCursor, SidebarSection};
        let inside = super::contains(self.hits.chat_overlay, point);
        if !inside {
            return match mouse.kind {
                MouseEventKind::Moved if self.sidebar_focus.hover_opened => {
                    self.leave_sidebar(false, outcome);
                    true
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    // Der Klick fährt ein und erreicht danach die Panes.
                    self.sidebar_focus.agent_query.clear();
                    self.leave_sidebar(false, outcome);
                    false
                }
                _ => false,
            };
        }
        let card = self
            .hits
            .agents
            .iter()
            .find(|(rect, _)| super::contains(*rect, point))
            .map(|(_, pane_id)| SidebarAgentCursor {
                endpoint_id: self.active_endpoint_id.clone(),
                pane_id: pane_id.clone(),
            });
        match mouse.kind {
            MouseEventKind::Moved if !self.sidebar_focus.editing => {
                if let Some(cursor) =
                    card.filter(|cursor| self.sidebar_focus.agent_cursor.as_ref() != Some(cursor))
                {
                    self.sidebar_focus.section = SidebarSection::Agents;
                    self.sidebar_focus.agent_cursor = Some(cursor);
                    outcome.repaint = true;
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(cursor) = card {
                    self.sidebar_focus.agent_cursor = Some(cursor);
                    self.commit_sidebar(outcome);
                    return true;
                }
                if super::contains(self.hits.agent_sort_toggle, point) {
                    self.toggle_chat_sort(outcome);
                    return true;
                }
                // `new` und `menu` laufen über die gewöhnliche Klickbehandlung.
                !(super::contains(self.hits.new_workspace, point)
                    || super::contains(self.hits.global_launcher, point))
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let next = if mouse.kind == MouseEventKind::ScrollUp {
                    self.agent_scroll.saturating_sub(1)
                } else {
                    self.agent_scroll
                        .saturating_add(1)
                        .min(self.hits.agent_max_scroll)
                };
                if next != self.agent_scroll {
                    self.agent_scroll = next;
                    outcome.repaint = true;
                }
                true
            }
            _ => true,
        }
    }

    /// Fährt nach Hover-Ablauf aus und lässt die Linie pulsieren, wenn ein Chat
    /// neu auf den Nutzer wartet.
    pub(crate) fn tick_chat_overlay(
        &mut self,
        now: std::time::Instant,
        outcome: &mut ClientShellInput,
    ) {
        if !self.chat_overlay_enabled() {
            self.chat_line_hover_deadline = None;
            self.chat_pulse_started = None;
            return;
        }
        if self
            .chat_line_hover_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.open_chat_overlay(true, outcome);
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let millis = crate::workspace::unix_millis_now();
        let latest = latest_attention(snapshot, millis).unwrap_or(0);
        // Der erste Snapshot legt nur den Ausgangswert fest.
        if self
            .chat_attention_latest
            .is_some_and(|previous| latest > previous)
        {
            self.chat_pulse_started = Some(millis);
            outcome.repaint = true;
        }
        self.chat_attention_latest = Some(latest);
        if self
            .chat_pulse_started
            .is_some_and(|started| millis.saturating_sub(started) >= PULSE_MS)
        {
            self.chat_pulse_started = None;
            outcome.repaint = true;
        }
    }

    /// Phase des Pulses für den Neuzeichnen-Schlüssel der Chat-Liste.
    pub(super) fn chat_pulse_phase(&self, now: u64) -> Option<u64> {
        self.chat_pulse_started
            .map(|started| now.saturating_sub(started) / PULSE_PHASE_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_width_follows_percent_within_bounds() {
        assert_eq!(overlay_rect(200, 40, 45).width, 90);
        assert_eq!(overlay_rect(80, 40, 45).width, 50);
        assert_eq!(overlay_rect(300, 40, 45).width, 100);
        assert_eq!(overlay_rect(40, 40, 45).width, 40);
    }

    #[test]
    fn percent_is_clamped() {
        assert_eq!(clamp_percent(10), 30);
        assert_eq!(clamp_percent(90), 70);
        assert_eq!(clamp_percent(45), 45);
    }

    #[test]
    fn pulse_alternates_three_times() {
        let start = 1_000;
        let bright = (0..8)
            .map(|phase| pulse_bright(Some(start), start + phase * PULSE_PHASE_MS))
            .collect::<Vec<_>>();
        assert_eq!(
            bright,
            [true, false, true, false, true, false, false, false]
        );
        assert!(!pulse_bright(None, start));
    }
}
