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
    /// Name des Agents (`claude`, `codex`, ...).
    pub(super) agent: Option<String>,
    /// Repo-Name: Basename des Agent-Verzeichnisses, sonst des Worktrees.
    pub(super) repo: Option<String>,
    pub(super) branch: Option<String>,
    pub(super) ahead_behind: Option<(usize, usize)>,
    /// Der Space ist ein verlinkter Worktree.
    pub(super) linked_worktree: bool,
    /// Tabs ab Nummer 2, die Nebenarbeit.
    pub(super) extra_tabs: usize,
    /// Seit wann der Agent arbeitet (Unix-Millisekunden).
    pub(super) working_since: Option<u64>,
    /// Der Chat wartet, und der Nutzer hat ihn seitdem nicht geöffnet.
    pub(super) unread: bool,
    pub(super) last_message: Option<String>,
    pub(super) blocked_reason: Option<String>,
    /// Worauf der Agent wartet, etwa `Formular: 7 Fragen`.
    pub(super) awaiting: Option<String>,
    /// Der gespeicherte Agent ist noch nicht gestartet.
    pub(super) stopped: bool,
    pub(super) context_tokens: Option<u64>,
    pub(super) context_window: Option<u64>,
    /// Abgeschlossene Arbeitsphasen der letzten 30 Minuten.
    pub(super) working_spans: Vec<(u64, u64)>,
    /// Der Chat ist ein Master Chat.
    pub(super) master: bool,
    /// Master eines Child-Chats.
    pub(super) parent: Option<String>,
    /// Auftrag eines Child-Chats.
    pub(super) assignment: Option<String>,
    /// Letzter Inbox-Eintrag eines Masters.
    pub(super) inbox_latest: Option<String>,
    /// Children eines Masters nach Status.
    pub(super) children: ChildCounts,
    /// Ein offenes Child, das eingerückt unter seinem Master steht.
    pub(super) nested: bool,
}

/// collie: Children eines Masters nach Status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ChildCounts {
    pub(super) total: usize,
    /// Blockiert oder wartet auf Antwort.
    pub(super) blocked: usize,
    pub(super) working: usize,
    /// Fertig oder idle.
    pub(super) done: usize,
    pub(super) stopped: usize,
}

impl ChildCounts {
    fn count(&mut self, row: &ChatRow) {
        self.total += 1;
        if row.stopped {
            self.stopped += 1;
        } else if row.needs_reply() {
            self.blocked += 1;
        } else if row.status == AgentStatus::Working {
            self.working += 1;
        } else {
            self.done += 1;
        }
    }
}

impl ChatRow {
    pub(super) fn snoozed(&self, now: u64) -> bool {
        self.snoozed_until.is_some_and(|until| until > now)
    }

    /// Versteckt sind laufende und gesnoozte Chats. Master stehen immer in
    /// der Hauptliste, solange sie nicht gesnoozt sind.
    pub(super) fn hidden(&self, now: u64) -> bool {
        (self.status == AgentStatus::Working && !self.master) || self.snoozed(now)
    }

    /// Der Agent wartet auf eine Antwort, etwa auf ein Formular. Solange er
    /// arbeitet oder blockiert, zählt sein Status.
    pub(super) fn awaiting_reply(&self) -> bool {
        self.awaiting.is_some()
            && !matches!(self.status, AgentStatus::Working | AgentStatus::Blocked)
    }

    /// Der Chat braucht den Nutzer dringend: blockiert oder wartet auf Antwort.
    /// Ein Master zählt auch, wenn eines seiner Children blockiert.
    pub(super) fn needs_reply(&self) -> bool {
        self.status == AgentStatus::Blocked || self.awaiting_reply() || self.children.blocked > 0
    }
}

/// Alle Chats eines Snapshots samt Children. Master tragen die Zähler ihrer
/// Children.
pub(super) fn all_chat_rows(snapshot: &ClientShellSnapshot) -> Vec<ChatRow> {
    let mut rows = snapshot
        .workspaces
        .iter()
        .filter_map(|workspace| chat_row(snapshot, workspace))
        .collect::<Vec<_>>();
    if rows.iter().any(|row| row.master) {
        let mut counts = std::collections::HashMap::<String, ChildCounts>::new();
        for row in &rows {
            if let Some(parent) = row.parent.as_ref() {
                counts.entry(parent.clone()).or_default().count(row);
            }
        }
        for row in rows.iter_mut().filter(|row| row.master) {
            row.children = counts.remove(&row.workspace_id).unwrap_or_default();
        }
    }
    rows
}

/// Children eines Masters.
pub(super) fn child_rows(
    snapshot: &ClientShellSnapshot,
    master_workspace_id: &str,
) -> Vec<ChatRow> {
    all_chat_rows(snapshot)
        .into_iter()
        .filter(|row| row.parent.as_deref() == Some(master_workspace_id))
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
    let repo = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .and_then(|pane| pane.foreground_cwd.as_deref().or(pane.cwd.as_deref()))
        .and_then(|cwd| std::path::Path::new(cwd).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            workspace
                .worktree
                .as_ref()
                .map(|worktree| worktree.label.clone())
        });
    let extra_tabs = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace.workspace_id && tab.tab_id != first_tab.tab_id)
        .count();
    // Ein gestoppter Agent kann schon als Agent bekannt sein, etwa mit dem
    // Namen aus der Session; sein Status zählt erst, wenn er läuft.
    let stopped_agent = workspace.stopped_agent.clone();
    Some(ChatRow {
        workspace_id: workspace.workspace_id.clone(),
        pane_id,
        status: agent.map_or(AgentStatus::Unknown, |agent| agent.agent_status),
        has_agent: agent.is_some() || stopped_agent.is_some(),
        label,
        attention_since: workspace.attention_since.unwrap_or(0),
        snoozed_until: workspace.snoozed_until,
        focused: workspace.focused,
        agent: agent
            .and_then(|agent| {
                agent
                    .display_agent
                    .clone()
                    .or_else(|| agent.agent.clone())
                    .or_else(|| agent.name.clone())
            })
            .or_else(|| stopped_agent.clone()),
        repo,
        branch: workspace.branch.clone(),
        ahead_behind: workspace.git_ahead_behind,
        linked_worktree: workspace
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.is_linked_worktree),
        extra_tabs,
        working_since: workspace.working_since,
        unread: workspace.attention_seen == Some(false),
        last_message: workspace.last_message.clone(),
        blocked_reason: workspace.blocked_reason.clone(),
        awaiting: workspace.awaiting.clone(),
        stopped: stopped_agent.is_some(),
        context_tokens: workspace.context_tokens,
        context_window: workspace.context_window,
        working_spans: workspace.working_spans.clone(),
        master: workspace.team.as_ref().is_some_and(|team| team.master),
        parent: workspace
            .team
            .as_ref()
            .and_then(|team| team.parent_workspace_id.clone()),
        assignment: workspace
            .team
            .as_ref()
            .and_then(|team| team.assignment.clone()),
        inbox_latest: workspace
            .team
            .as_ref()
            .and_then(|team| team.inbox_latest.clone()),
        children: ChildCounts::default(),
        nested: false,
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
    let all = all_chat_rows(snapshot);
    let mut rows = all
        .iter()
        .filter(|row| row.parent.is_none())
        .filter(|row| row.hidden(now) == show_hidden)
        .filter(|row| super::sidebar_focus::chat_matches_filter(row, query))
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.attention_since);
    if newest_first {
        rows.reverse();
    }
    // Master stehen oben angepinnt.
    rows.sort_by_key(|row| !row.master);
    // Ein offenes Child steht eingerückt unter seinem Master, solange es offen ist.
    if !show_hidden {
        if let Some(child) = all.iter().find(|row| row.focused && row.parent.is_some()) {
            if let Some(index) = rows
                .iter()
                .position(|row| Some(&row.workspace_id) == child.parent.as_ref())
            {
                let mut child = child.clone();
                child.nested = true;
                rows.insert(index + 1, child);
            }
        }
    }
    rows
}

/// collie: Hauptliste und unterer Abschnitt des Chat-Overlays. Der untere
/// Abschnitt hält laufende und gesnoozte Chats.
pub(super) fn overlay_chat_rows(
    snapshot: &ClientShellSnapshot,
    newest_first: bool,
    query: &str,
    now: u64,
) -> (Vec<ChatRow>, Vec<ChatRow>) {
    (
        visible_chat_rows(snapshot, false, newest_first, query, now),
        visible_chat_rows(snapshot, true, newest_first, query, now),
    )
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Dauer eines Spinner-Frames.
pub(super) const SPINNER_FRAME_MS: u64 = 100;

pub(super) fn spinner_frame(now: u64) -> &'static str {
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

/// Symbol eines gestoppten Chats.
pub(super) const SYMBOL_STOPPED: &str = "■";
/// Symbol eines Chats, der auf eine Antwort wartet.
pub(super) const SYMBOL_AWAITING: &str = "?";

/// Symbol und Stil einer Zeile. Nur „du bist dran", „arbeitet" und
/// „gestoppt" haben ein Symbol.
pub(super) fn chat_symbol(
    row: &ChatRow,
    now: u64,
    palette: &Palette,
) -> Option<(&'static str, Style)> {
    if row.stopped {
        return Some((SYMBOL_STOPPED, Style::default().fg(palette.overlay0)));
    }
    // Ein Master mit blockiertem Child zählt selbst als blockiert.
    if row.children.blocked > 0 && row.status != AgentStatus::Blocked {
        return Some((
            "●",
            Style::default().fg(status_color(AgentStatus::Blocked, palette)),
        ));
    }
    if row.awaiting_reply() {
        return Some((
            SYMBOL_AWAITING,
            Style::default()
                .fg(palette.yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
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
    // collie: Zähler der globalen Nutzer-Inbox hinter dem Titel.
    let mut title_end = area.x + 1 + display_width(title);
    if let Some(badge) = super::user_inbox::inbox_badge(snapshot, palette) {
        let width = super::user_inbox::badge_width(&badge);
        if sort_rect.x.saturating_sub(title_end) > width + 2 {
            let x = title_end.saturating_add(1);
            super::user_inbox::put_spans(buffer, x, area.y, sort_rect.x, &badge);
            hits.user_inbox_counts.push(Rect::new(x, area.y, width, 1));
            title_end = x.saturating_add(width);
        }
    }
    if let Some(filter) = view.filter_label(super::sidebar_focus::SidebarSection::Agents) {
        let label = format!(" {filter} ");
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
    let remaining_style = Style::default().fg(palette.overlay0);
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
    let label = if row.master {
        format!(
            "{} {} ({})",
            super::chat_cards::ICON_CROWN,
            row.label,
            row.children.total
        )
    } else if row.nested {
        format!("└ {}", row.label)
    } else {
        row.label.clone()
    };
    put_text(buffer, label_x, rect.y, label_width, &label, label_style);
    if let Some(remaining) = remaining {
        put_text(
            buffer,
            rect.right().saturating_sub(remaining_width),
            rect.y,
            remaining_width,
            &remaining,
            remaining_style,
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
