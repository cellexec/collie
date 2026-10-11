//! collie: Chat-Liste für `ui.sidebar.agents_only`.
//!
//! Ein Chat ist ein Space. Tab 1 hält den Agent, weitere Tabs sind
//! Nebenarbeit. Die Hauptliste zeigt nur Chats, bei denen der Nutzer dran ist;
//! laufende und gesnoozte Chats stehen in der versteckten Ansicht.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::AgentStatus;

/// Eine Zeile der Chat-Liste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChatRow {
    pub(super) workspace_id: String,
    /// Pane, das Öffnen fokussiert: der Agent in Tab 1, sonst das erste Pane
    /// von Tab 1.
    pub(super) pane_id: String,
    pub(super) status: AgentStatus,
    pub(super) has_agent: bool,
    pub(super) label: String,
    pub(super) attention_since: u64,
    pub(super) snoozed_until: Option<u64>,
    pub(super) focused: bool,
}

impl ChatRow {
    pub(super) fn snoozed(&self, now: u64) -> bool {
        self.snoozed_until.is_some_and(|until| until > now)
    }

    /// Versteckt sind laufende und gesnoozte Chats.
    pub(super) fn hidden(&self, now: u64) -> bool {
        self.status == AgentStatus::Working || self.snoozed(now)
    }
}

/// Alle Chats eines Snapshots, unsortiert und ungefiltert.
pub(super) fn chat_rows(snapshot: &ClientShellSnapshot) -> Vec<ChatRow> {
    snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| chat_row(snapshot, workspace))
        .collect()
}

fn chat_row(snapshot: &ClientShellSnapshot, workspace: &ClientShellWorkspace) -> Option<ChatRow> {
    let first_tab = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace.workspace_id)
        .min_by_key(|tab| tab.number)?;
    let tab_panes = snapshot
        .panes
        .iter()
        .filter(|pane| pane.tab_id == first_tab.tab_id)
        .collect::<Vec<_>>();
    let agent = tab_panes.iter().find_map(|pane| {
        snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane.pane_id)
    });
    let pane_id = agent
        .map(|agent| agent.pane_id.clone())
        .or_else(|| tab_panes.first().map(|pane| pane.pane_id.clone()))?;
    let title = agent
        .and_then(|agent| {
            agent
                .terminal_title_stripped
                .as_deref()
                .or(agent.title.as_deref())
        })
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let label = match title {
        Some(title) if !workspace.custom_label => title.to_owned(),
        _ => workspace.label.clone(),
    };
    Some(ChatRow {
        workspace_id: workspace.workspace_id.clone(),
        pane_id,
        status: agent.map_or(AgentStatus::Unknown, |agent| agent.agent_status),
        has_agent: agent.is_some(),
        label,
        attention_since: workspace.attention_since.unwrap_or(0),
        snoozed_until: workspace.snoozed_until,
        focused: workspace.focused,
    })
}

/// Die Chats einer Ansicht in Anzeigereihenfolge.
pub(super) fn visible_chat_rows(
    snapshot: &ClientShellSnapshot,
    show_hidden: bool,
    newest_first: bool,
    query: &str,
    now: u64,
) -> Vec<ChatRow> {
    let mut rows = chat_rows(snapshot)
        .into_iter()
        .filter(|row| row.hidden(now) == show_hidden)
        .filter(|row| super::sidebar_focus::chat_matches_filter(row, query))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.attention_since);
    if newest_first {
        rows.reverse();
    }
    rows
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Dauer eines Spinner-Frames.
pub(super) const SPINNER_FRAME_MS: u64 = 100;

fn spinner_frame(now: u64) -> &'static str {
    SPINNER[((now / SPINNER_FRAME_MS) % SPINNER.len() as u64) as usize]
}

/// Restzeit eines Snooze, grob gerundet: `45m`, `3h`, `2d`.
pub(super) fn snooze_remaining_label(until: u64, now: u64) -> String {
    let minutes = until.saturating_sub(now).div_ceil(60_000);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h", minutes.div_ceil(60))
    } else {
        format!("{}d", minutes.div_ceil(24 * 60))
    }
}

/// Symbol und Stil einer Zeile. Nur „du bist dran" und „arbeitet" haben ein
/// Symbol.
fn chat_symbol(row: &ChatRow, now: u64, palette: &Palette) -> Option<(&'static str, Style)> {
    match row.status {
        AgentStatus::Working => Some((
            spinner_frame(now),
            Style::default().fg(status_color(AgentStatus::Working, palette)),
        )),
        AgentStatus::Blocked => Some((
            "●",
            Style::default().fg(status_color(AgentStatus::Blocked, palette)),
        )),
        AgentStatus::Done | AgentStatus::Idle => Some(("●", Style::default().fg(palette.accent))),
        AgentStatus::Unknown => None,
    }
}

/// Zeichnet die Chat-Liste in den Rahmen `area` und meldet die Zeilen als
/// Treffer in `hits.agents`.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chat_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    view: super::sidebar_focus::SidebarRenderView<'_>,
    endpoint_id: &ClientEndpointId,
    newest_first: bool,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if area.height < 3 {
        return;
    }
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let title = if view.show_hidden {
        " hidden "
    } else {
        " chats "
    };
    let active = view.section_active(super::sidebar_focus::SidebarSection::Agents);
    super::render::sidebar::render_section_box(buffer, area, title, active, None, palette);

    let sort_label = if newest_first { " newest " } else { " oldest " };
    let sort_width = display_width(sort_label).min(area.width.saturating_sub(1));
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    if let Some(filter) = view.filter_label(super::sidebar_focus::SidebarSection::Agents) {
        let label = format!(" {filter} ");
        let title_end = area.x + 1 + display_width(title);
        let available = sort_rect.x.saturating_sub(title_end + 1);
        if available > 2 {
            let width = display_width(&label).min(available);
            put_text(
                buffer,
                sort_rect.x.saturating_sub(width),
                area.y,
                width,
                &label,
                Style::default().fg(palette.accent),
            );
        }
    }

    let rows = visible_chat_rows(
        snapshot,
        view.show_hidden,
        newest_first,
        view.agent_query,
        now,
    );
    let empty = if view.show_hidden {
        " nothing hidden"
    } else {
        " all caught up"
    };
    super::agent_sidebar::render_agent_list(
        buffer,
        area,
        &rows,
        Some(empty),
        config,
        agent_scroll,
        hits,
        |_| 1,
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.pane_id.clone()));
            render_chat_row(buffer, rect, row, now, palette);
            if view.agent_selected(endpoint_id, &row.pane_id) {
                buffer.set_style(rect, Style::default().bg(palette.selection_bg));
            } else if row.focused {
                buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
            }
        },
    );
}

fn render_chat_row(buffer: &mut Buffer, rect: Rect, row: &ChatRow, now: u64, palette: &Palette) {
    let snoozed = row.snoozed(now);
    if let Some((symbol, style)) = chat_symbol(row, now, palette) {
        let style = if snoozed {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        };
        put_text(buffer, rect.x.saturating_add(1), rect.y, 1, symbol, style);
    }
    let remaining = row
        .snoozed_until
        .filter(|_| snoozed)
        .map(|until| format!(" {}", snooze_remaining_label(until, now)));
    let remaining_width = remaining.as_deref().map_or(0, display_width);
    let label_x = rect.x.saturating_add(3);
    let label_width = rect
        .right()
        .saturating_sub(label_x)
        .saturating_sub(remaining_width);
    let label_style = if snoozed {
        Style::default().fg(palette.overlay0)
    } else if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.subtext0)
    };
    put_text(
        buffer,
        label_x,
        rect.y,
        label_width,
        &row.label,
        label_style,
    );
    if let Some(remaining) = remaining {
        put_text(
            buffer,
            rect.right().saturating_sub(remaining_width),
            rect.y,
            remaining_width,
            &remaining,
            Style::default().fg(palette.overlay0),
        );
    }
}

/// Eingeklappte Sidebar: Nummer und Symbol der Hauptliste.
pub(super) fn render_collapsed_chats(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    newest_first: bool,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let rows = visible_chat_rows(snapshot, false, newest_first, "", now);
    for (index, row) in rows.iter().take(area.height as usize).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        if row.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &format!("{:<2}", index + 1),
            Style::default().fg(if row.focused {
                palette.text
            } else {
                palette.overlay0
            }),
        );
        if let Some((symbol, style)) = chat_symbol(row, now, palette) {
            put_text(
                buffer,
                rect.x.saturating_add(2),
                rect.y,
                rect.width.saturating_sub(2),
                symbol,
                style,
            );
        }
        hits.agents.push((rect, row.pane_id.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_remaining_label_rounds_up() {
        let now = 1_000_000;
        assert_eq!(snooze_remaining_label(now + 30_000, now), "1m");
        assert_eq!(snooze_remaining_label(now + 45 * 60_000, now), "45m");
        assert_eq!(snooze_remaining_label(now + 61 * 60_000, now), "2h");
        assert_eq!(snooze_remaining_label(now + 3 * 3_600_000, now), "3h");
        assert_eq!(snooze_remaining_label(now + 30 * 3_600_000, now), "2d");
    }
}
