//! collie: Inhalt des Chat-Overlays. Jeder wartende Chat ist eine Karte mit
//! drei Zeilen; laufende und gesnoozte Chats stehen als Einzeiler darunter.
//!
//! Gezeichnet wird nur aus Snapshot-Feldern. Kein Bildschirm-Read, kein
//! Dateizugriff.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::chat_visuals::{
    activity_buckets, blend, context_color, context_gauge, context_ratio, doze_frame, heat_color,
    pulse_amount, repo_color, repo_initials, spark_symbol, COLLIE, COLLIE_WIDTH, SPARK_BUCKETS,
};
use super::chats::{
    chat_symbol, overlay_chat_rows, snooze_remaining_label, spinner_frame, ChatRow,
};
use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::AgentStatus;
use crate::ui::truncate_end;

/// Zeilen einer Karte.
pub(super) const CARD_HEIGHT: u16 = 3;
/// Leerzeilen zwischen zwei Karten.
pub(super) const CARD_GAP: u16 = 1;

const ICON_AGENT: &str = "󰚩";
const ICON_SHELL: &str = "";
const ICON_REPO: &str = "";
const ICON_BRANCH: &str = "";
const ICON_WORKTREE: &str = "";
const ICON_SNOOZE: &str = "󰒲";
const ICON_DONE: &str = "";
/// So stark färbt das Nachleuchten den Kartenhintergrund höchstens.
const PULSE_TINT: f32 = 0.35;
const ICON_MARKED: &str = "✓";

/// Ab hier wird die Wartezeit gelb, dann orange.
const AGE_WARM_MS: u64 = 15 * 60_000;
const AGE_HOT_MS: u64 = 60 * 60_000;

/// Wartezeit grob gerundet: `0m`, `12m`, `3h`, `2d`.
pub(super) fn age_label(since: u64, now: u64) -> String {
    let minutes = now.saturating_sub(since) / 60_000;
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 24 * 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}d", minutes / (24 * 60))
    }
}

/// Farbe der Wartezeit nach Alter: grau, gelb ab 15 Minuten, orange ab 1 Stunde.
pub(super) fn age_color(since: u64, now: u64, palette: &Palette) -> Color {
    let age = now.saturating_sub(since);
    if age >= AGE_HOT_MS {
        palette.peach
    } else if age >= AGE_WARM_MS {
        palette.yellow
    } else {
        palette.overlay0
    }
}

/// Höhe des unteren Abschnitts: Kopfzeile plus Einzeiler, höchstens ein
/// Drittel der Fläche.
fn hidden_section_height(rows: usize, available: u16) -> u16 {
    if rows == 0 || available < 6 {
        return 0;
    }
    let wanted = u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(1);
    wanted.min((available / 3).max(2))
}

/// Zeichnet die Chat-Karten in `area` und meldet sie als Treffer.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_chat_cards(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    view: super::sidebar_focus::SidebarRenderView<'_>,
    endpoint_id: &ClientEndpointId,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if area.height < 4 || area.width < 8 {
        return;
    }
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    let (main, hidden) =
        overlay_chat_rows(snapshot, config.chat_newest_first, view.agent_query, now);
    let selected = |row: &ChatRow| view.agent_selected(endpoint_id, &row.pane_id);
    let marked = |row: &ChatRow| view.agent_marked(endpoint_id, &row.pane_id);

    render_header(buffer, area, config, &view, hits);
    let marks = main.iter().chain(&hidden).filter(|row| marked(row)).count();
    if marks > 0 {
        let label = format!("{ICON_MARKED} {marks} marked ");
        let width = display_width(&label);
        let x = area
            .right()
            .saturating_sub(width)
            .saturating_sub(display_width(" newest "));
        put_text(
            buffer,
            x,
            area.y,
            width,
            &label,
            Style::default().fg(palette.green),
        );
    }

    // Kopfzeile, Leerzeile, Inhalt, Fußzeile mit `new` und `menu`.
    let content = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2),
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    let hidden_height = hidden_section_height(hidden.len(), content.height);
    let main_body = Rect::new(
        content.x,
        content.y,
        content.width,
        content
            .height
            .saturating_sub(hidden_height)
            .saturating_sub(u16::from(hidden_height > 0)),
    );
    hits.agent_body = main_body;

    if main.is_empty() {
        *agent_scroll = 0;
        render_empty(buffer, main_body, &hidden, now, palette);
    } else {
        render_main_cards(
            buffer,
            main_body,
            &main,
            now,
            palette,
            agent_scroll,
            hits,
            &selected,
            &marked,
        );
    }

    if hidden_height > 0 {
        let section = Rect::new(
            content.x,
            content.bottom().saturating_sub(hidden_height),
            content.width,
            hidden_height,
        );
        render_hidden_section(
            buffer, section, &hidden, now, palette, hits, &selected, &marked,
        );
    }
}

fn render_header(
    buffer: &mut Buffer,
    area: Rect,
    config: &ClientShellConfig,
    view: &super::sidebar_focus::SidebarRenderView<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let title_style = Style::default()
        .fg(if view.focused {
            palette.accent
        } else {
            palette.text
        })
        .add_modifier(Modifier::BOLD);
    let title = " 󰭹 chats";
    put_text(buffer, area.x, area.y, area.width, title, title_style);

    let sort_label = if config.chat_newest_first {
        " newest "
    } else {
        " oldest "
    };
    let sort_width = display_width(sort_label).min(area.width);
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
        Style::default().fg(palette.overlay0),
    );

    if let Some(filter) = view.filter_label(super::sidebar_focus::SidebarSection::Agents) {
        let label = format!(" {filter} ");
        let start = area
            .x
            .saturating_add(display_width(title))
            .saturating_add(1);
        let available = sort_rect.x.saturating_sub(start);
        put_text(
            buffer,
            start,
            area.y,
            available,
            &label,
            Style::default().fg(palette.accent),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_main_cards(
    buffer: &mut Buffer,
    body: Rect,
    rows: &[ChatRow],
    now: u64,
    palette: &Palette,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    selected: &impl Fn(&ChatRow) -> bool,
    marked: &impl Fn(&ChatRow) -> bool,
) {
    if body.height < CARD_HEIGHT {
        return;
    }
    let heights = vec![CARD_HEIGHT; rows.len()];
    let mut gaps = vec![CARD_GAP; rows.len()];
    if let Some(last) = gaps.last_mut() {
        *last = 0;
    }
    let metrics = super::scroll::list_scroll_metrics(&heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0;
    let width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for row in rows.iter().skip(*agent_scroll) {
        if y.saturating_add(CARD_HEIGHT) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, width, CARD_HEIGHT);
        render_card(buffer, rect, row, selected(row), marked(row), now, palette);
        hits.agents.push((rect, row.pane_id.clone()));
        y = y.saturating_add(CARD_HEIGHT + CARD_GAP);
    }
    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }
}

/// Eine Karte: Repo und Branch; Status, Titel und Wartezeit; Grund oder
/// Vorschau.
fn render_card(
    buffer: &mut Buffer,
    rect: Rect,
    row: &ChatRow,
    selected: bool,
    marked: bool,
    now: u64,
    palette: &Palette,
) {
    let resting = if selected {
        palette.surface0
    } else {
        palette.surface_dim
    };
    // Ein Chat, der gerade neu wartet, leuchtet kurz in seiner Statusfarbe.
    let pulse_color = if row.needs_reply() {
        status_color(AgentStatus::Blocked, palette)
    } else {
        palette.accent
    };
    let background = blend(
        resting,
        pulse_color,
        PULSE_TINT * pulse_amount(row.attention_since, now),
    );
    buffer.set_style(rect, Style::default().bg(background));
    let base = Style::default().bg(background);
    // Linker Rand: Cursor in Akzentfarbe, sonst ein Streifen, der mit der
    // Wartezeit wärmer wird.
    let stripe = if selected {
        palette.accent
    } else {
        heat_color(row.attention_since, now, palette)
    };
    for y in rect.y..rect.bottom() {
        put_text(buffer, rect.x, y, 1, "▌", base.fg(stripe));
    }
    if marked {
        put_text(
            buffer,
            rect.x,
            rect.y,
            1,
            ICON_MARKED,
            base.fg(palette.green),
        );
    }
    let inner_x = rect.x.saturating_add(2);
    let right = rect.right().saturating_sub(1);
    let muted = base.fg(palette.overlay0);

    // Zeile 1: Repo links, Branch rechts, davor Aktivität und Kontext.
    let first_y = rect.y;
    let branch = row.branch.as_deref().map(|branch| {
        let mut label = format!("{ICON_BRANCH} {branch}");
        if let Some((ahead, behind)) = row.ahead_behind {
            if ahead > 0 {
                label.push_str(&format!(" ↑{ahead}"));
            }
            if behind > 0 {
                label.push_str(&format!(" ↓{behind}"));
            }
        }
        label
    });
    let mut line_end = right;
    if let Some(branch) = branch.as_deref() {
        // Der Branch bekommt höchstens die halbe Zeile.
        let width = display_width(branch).min(right.saturating_sub(inner_x) / 2);
        let branch = truncate_end(branch, usize::from(width));
        put_text(
            buffer,
            right.saturating_sub(width),
            first_y,
            width,
            &branch,
            base.fg(palette.mauve),
        );
        line_end = right.saturating_sub(width).saturating_sub(2);
    }
    let line_end = render_card_meters(buffer, inner_x, first_y, line_end, row, now, palette, base);
    let mut x = inner_x;
    if let Some(repo) = row.repo.as_deref() {
        // Jedes Repo hat einen festen Farbton: Kürzel als Chip, Name in
        // derselben Farbe.
        let hue = repo_color(repo, palette);
        let initials = repo_initials(repo);
        x = if initials.is_empty() {
            put_part(buffer, x, first_y, line_end, ICON_REPO, muted)
        } else {
            put_part(
                buffer,
                x,
                first_y,
                line_end,
                &format!(" {initials} "),
                base.fg(background).bg(hue).add_modifier(Modifier::BOLD),
            )
        };
        x = put_part(
            buffer,
            x.saturating_add(1),
            first_y,
            line_end,
            repo,
            base.fg(hue).add_modifier(Modifier::BOLD),
        );
    } else {
        let icon = if row.has_agent {
            ICON_AGENT
        } else {
            ICON_SHELL
        };
        x = put_part(
            buffer,
            x,
            first_y,
            line_end,
            icon,
            base.fg(palette.overlay1),
        );
        if let Some(agent) = row.agent.as_deref() {
            x = put_part(buffer, x.saturating_add(1), first_y, line_end, agent, muted);
        }
    }
    if row.linked_worktree {
        x = put_part(
            buffer,
            x.saturating_add(2),
            first_y,
            line_end,
            ICON_WORKTREE,
            muted,
        );
    }
    if row.extra_tabs > 0 {
        let shells = format!("{ICON_SHELL} +{}", row.extra_tabs);
        put_part(
            buffer,
            x.saturating_add(2),
            first_y,
            line_end,
            &shells,
            muted,
        );
    }

    // Zeile 2: Status, Titel, Wartezeit. Ohne bekannte Wartezeit (älterer
    // Server) bleibt sie leer.
    let title_y = rect.y.saturating_add(1);
    let age = if row.attention_since == 0 {
        String::new()
    } else {
        age_label(row.attention_since, now)
    };
    let age_width = display_width(&age);
    put_text(
        buffer,
        right.saturating_sub(age_width),
        title_y,
        age_width,
        &age,
        base.fg(age_color(row.attention_since, now, palette)),
    );
    if let Some((symbol, style)) = chat_symbol(row, now, palette) {
        // Gelesene Chats bekommen ein hohles Symbol.
        let symbol = if row.unread || symbol != "●" {
            symbol
        } else {
            "○"
        };
        put_text(buffer, inner_x, title_y, 1, symbol, style.bg(background));
    }
    let title_x = inner_x.saturating_add(2);
    let title_width = right
        .saturating_sub(age_width)
        .saturating_sub(1)
        .saturating_sub(title_x);
    let title_style = if row.unread || row.focused || selected {
        base.fg(palette.text).add_modifier(Modifier::BOLD)
    } else {
        base.fg(palette.subtext0)
    };
    put_text(
        buffer,
        title_x,
        title_y,
        title_width,
        &truncate_end(&row.label, usize::from(title_width)),
        title_style,
    );

    // Zeile 3: bei Blocked der Grund, bei einer offenen Rückfrage deren
    // Thema, sonst die letzte Nachricht.
    let detail_y = rect.y.saturating_add(2);
    let detail_width = right.saturating_sub(inner_x);
    let (detail, detail_style) = if row.awaiting_reply() {
        (
            format!(
                "wartet auf Antwort: {}",
                row.awaiting.as_deref().unwrap_or_default()
            ),
            base.fg(palette.yellow),
        )
    } else if row.stopped {
        (
            match row.last_message.as_deref() {
                Some(message) => format!("gestoppt: {message}"),
                None => "gestoppt, enter startet den Agent".to_owned(),
            },
            base.fg(palette.overlay0).add_modifier(Modifier::ITALIC),
        )
    } else if row.status == AgentStatus::Blocked {
        (
            row.blocked_reason
                .clone()
                .unwrap_or_else(|| "wartet auf Eingabe".to_owned()),
            base.fg(status_color(AgentStatus::Blocked, palette)),
        )
    } else if let Some(message) = row.last_message.as_deref() {
        (
            message.to_owned(),
            base.fg(palette.overlay1).add_modifier(Modifier::ITALIC),
        )
    } else {
        return;
    };
    put_text(
        buffer,
        inner_x,
        detail_y,
        detail_width,
        &truncate_end(&detail, usize::from(detail_width)),
        detail_style,
    );
}

/// Zeichnet rechtsbündig Aktivitätskurve und Kontext-Balken, soweit Platz
/// ist, und gibt den rechten Rand für den Rest der Zeile zurück.
#[allow(clippy::too_many_arguments)]
fn render_card_meters(
    buffer: &mut Buffer,
    left: u16,
    y: u16,
    right: u16,
    row: &ChatRow,
    now: u64,
    palette: &Palette,
    base: Style,
) -> u16 {
    // Links soll Platz für Agent, Repo und Branch bleiben.
    const LEFT_RESERVE: u16 = 24;
    let mut end = right;
    if let Some(ratio) = context_ratio(row.context_tokens, row.context_window) {
        let percent = format!(" {:>2}%", (ratio * 100.0).round() as u32);
        let gauge = context_gauge(ratio);
        let width = display_width(&gauge) + display_width(&percent);
        if end.saturating_sub(left) >= LEFT_RESERVE + width {
            let x = end - width;
            let color = context_color(ratio, palette);
            let gauge_width = display_width(&gauge);
            put_text(buffer, x, y, gauge_width, &gauge, base.fg(color));
            put_text(
                buffer,
                x + gauge_width,
                y,
                display_width(&percent),
                &percent,
                base.fg(palette.overlay0),
            );
            end = x.saturating_sub(2);
        }
    }
    let width = SPARK_BUCKETS as u16;
    if end.saturating_sub(left) >= LEFT_RESERVE + width {
        let x = end - width;
        if render_sparkline(buffer, x, y, row, now, palette, base) {
            end = x.saturating_sub(2);
        }
    }
    end
}

/// Aktivitätskurve der letzten 30 Minuten ab `x`. Ohne jede Arbeit im
/// Fenster bleibt die Fläche leer; dann kommt `false` zurück.
fn render_sparkline(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    row: &ChatRow,
    now: u64,
    palette: &Palette,
    base: Style,
) -> bool {
    let working_since = row
        .working_since
        .filter(|_| row.status == AgentStatus::Working);
    let buckets = activity_buckets(&row.working_spans, working_since, now);
    if buckets.iter().all(|value| *value <= 0.0) {
        return false;
    }
    let active = status_color(AgentStatus::Working, palette);
    for (offset, value) in buckets.iter().enumerate() {
        let (symbol, color) = match spark_symbol(*value) {
            Some(symbol) => (symbol, active),
            None => ("▁", palette.surface1),
        };
        put_text(buffer, x + offset as u16, y, 1, symbol, base.fg(color));
    }
    true
}

/// Schreibt `text` ab `x`, höchstens bis `right`, und gibt das Ende zurück.
fn put_part(buffer: &mut Buffer, x: u16, y: u16, right: u16, text: &str, style: Style) -> u16 {
    if x >= right {
        return x;
    }
    let width = display_width(text).min(right - x);
    put_text(buffer, x, y, width, text, style);
    x.saturating_add(width)
}

fn render_empty(buffer: &mut Buffer, body: Rect, hidden: &[ChatRow], now: u64, palette: &Palette) {
    if body.height < 3 {
        return;
    }
    let running = hidden
        .iter()
        .filter(|row| row.status == AgentStatus::Working && !row.snoozed(now))
        .count();
    let snoozed = hidden.iter().filter(|row| row.snoozed(now)).count();
    let summary = format!("läuft {running}  snooze {snoozed}");
    if body.height >= 10 && body.width >= COLLIE_WIDTH + 2 {
        render_dozing_collie(buffer, body, &summary, now, palette);
        return;
    }
    let lines = [
        (ICON_DONE.to_owned(), Style::default().fg(palette.green)),
        (
            "Nichts zu tun".to_owned(),
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        ),
        (summary, Style::default().fg(palette.overlay0)),
    ];
    let top = body.y + body.height.saturating_sub(lines.len() as u16) / 2;
    for (offset, (text, style)) in lines.iter().enumerate() {
        let width = display_width(text).min(body.width);
        put_text(
            buffer,
            body.x + body.width.saturating_sub(width) / 2,
            top + offset as u16,
            width,
            text,
            *style,
        );
    }
}

/// Leerer Zustand mit Platz: Die Collie döst, darunter steht, was noch läuft.
fn render_dozing_collie(
    buffer: &mut Buffer,
    body: Rect,
    summary: &str,
    now: u64,
    palette: &Palette,
) {
    // Zwei Zeilen `z`, die Collie, eine Leerzeile, Titel und Zusammenfassung.
    let height = 2 + COLLIE.len() as u16 + 3;
    let top = body.y + body.height.saturating_sub(height) / 2;
    let art_x = body.x + body.width.saturating_sub(COLLIE_WIDTH) / 2;
    let doze = Style::default().fg(palette.accent);
    for (offset, line) in doze_frame(now).iter().enumerate() {
        put_text(buffer, art_x, top + offset as u16, COLLIE_WIDTH, line, doze);
    }
    let art_style = Style::default().fg(palette.overlay1);
    for (offset, line) in COLLIE.iter().enumerate() {
        put_text(
            buffer,
            art_x,
            top + 2 + offset as u16,
            COLLIE_WIDTH,
            line,
            art_style,
        );
    }
    let lines = [
        (
            "alles erledigt",
            Style::default()
                .fg(palette.text)
                .add_modifier(Modifier::BOLD),
        ),
        (summary, Style::default().fg(palette.overlay0)),
    ];
    let text_top = top + 2 + COLLIE.len() as u16 + 1;
    for (offset, (text, style)) in lines.iter().enumerate() {
        let width = display_width(text).min(body.width);
        put_text(
            buffer,
            body.x + body.width.saturating_sub(width) / 2,
            text_top + offset as u16,
            width,
            text,
            *style,
        );
    }
}

// Die Zeichen-Helfer dieses Moduls reichen Stil und Treffer einzeln durch.
#[allow(clippy::too_many_arguments)]
fn render_hidden_section(
    buffer: &mut Buffer,
    section: Rect,
    rows: &[ChatRow],
    now: u64,
    palette: &Palette,
    hits: &mut ShellHitMap,
    selected: &impl Fn(&ChatRow) -> bool,
    marked: &impl Fn(&ChatRow) -> bool,
) {
    let running = rows
        .iter()
        .filter(|row| row.status == AgentStatus::Working && !row.snoozed(now))
        .count();
    let snoozed = rows.iter().filter(|row| row.snoozed(now)).count();
    let header = format!("läuft {running}  snooze {snoozed} ");
    let header_width = display_width(&header).min(section.width);
    for x in section.x..section.right() {
        if let Some(cell) = buffer.cell_mut((x, section.y)) {
            cell.set_symbol("─")
                .set_style(Style::default().fg(palette.surface1));
        }
    }
    put_text(
        buffer,
        section.x,
        section.y,
        header_width,
        &header,
        Style::default().fg(palette.overlay0),
    );

    let body = Rect::new(
        section.x,
        section.y.saturating_add(1),
        section.width,
        section.height.saturating_sub(1),
    );
    let visible = usize::from(body.height);
    let cursor = rows.iter().position(selected).unwrap_or(0);
    let start = (cursor + 1).saturating_sub(visible);
    for (offset, row) in rows.iter().skip(start).take(visible).enumerate() {
        let rect = Rect::new(body.x, body.y + offset as u16, body.width, 1);
        render_hidden_row(buffer, rect, row, selected(row), now, palette);
        if marked(row) {
            put_text(
                buffer,
                rect.x,
                rect.y,
                1,
                ICON_MARKED,
                Style::default().fg(palette.green),
            );
        }
        hits.agents.push((rect, row.pane_id.clone()));
    }
}

fn render_hidden_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &ChatRow,
    selected: bool,
    now: u64,
    palette: &Palette,
) {
    let base = if selected {
        Style::default().bg(palette.surface0)
    } else {
        Style::default()
    };
    buffer.set_style(rect, base);
    if selected {
        put_text(buffer, rect.x, rect.y, 1, "▌", base.fg(palette.accent));
    }
    let snoozed = row.snoozed(now);
    let (symbol, symbol_style) = if snoozed {
        (ICON_SNOOZE, base.fg(palette.overlay0))
    } else {
        (
            spinner_frame(now),
            base.fg(status_color(AgentStatus::Working, palette)),
        )
    };
    put_text(
        buffer,
        rect.x.saturating_add(2),
        rect.y,
        1,
        symbol,
        symbol_style,
    );
    // Rechts steht die Snooze-Restzeit oder die Laufzeit.
    let right_label = if snoozed {
        row.snoozed_until
            .map(|until| snooze_remaining_label(until, now))
            .unwrap_or_default()
    } else {
        row.working_since
            .map(|since| age_label(since, now))
            .unwrap_or_default()
    };
    let right_width = display_width(&right_label);
    let right = rect.right().saturating_sub(1);
    put_text(
        buffer,
        right.saturating_sub(right_width),
        rect.y,
        right_width,
        &right_label,
        base.fg(palette.overlay0),
    );
    let label_x = rect.x.saturating_add(4);
    let mut label_end = right.saturating_sub(right_width).saturating_sub(1);
    // Laufende Chats zeigen ihre Aktivität vor der Laufzeit, wenn der Titel
    // genug Platz behält.
    let spark_width = SPARK_BUCKETS as u16;
    if !snoozed && label_end.saturating_sub(label_x) >= 20 + spark_width {
        let spark_x = label_end - spark_width;
        if render_sparkline(buffer, spark_x, rect.y, row, now, palette, base) {
            label_end = spark_x.saturating_sub(1);
        }
    }
    let label_width = label_end.saturating_sub(label_x);
    put_text(
        buffer,
        label_x,
        rect.y,
        label_width,
        &row.label,
        base.fg(if selected {
            palette.text
        } else {
            palette.overlay1
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_label_rounds_down() {
        let now = 10 * 24 * 3_600_000;
        assert_eq!(age_label(now - 30_000, now), "0m");
        assert_eq!(age_label(now - 12 * 60_000, now), "12m");
        assert_eq!(age_label(now - 3 * 3_600_000 - 1, now), "3h");
        assert_eq!(age_label(now - 2 * 24 * 3_600_000, now), "2d");
        assert_eq!(age_label(now + 5_000, now), "0m");
    }

    #[test]
    fn age_color_warms_with_age() {
        let palette = Palette::catppuccin();
        let now = 10 * 3_600_000;
        assert_eq!(age_color(now - 60_000, now, &palette), palette.overlay0);
        assert_eq!(age_color(now - 15 * 60_000, now, &palette), palette.yellow);
        assert_eq!(age_color(now - 60 * 60_000, now, &palette), palette.peach);
    }

    #[test]
    fn hidden_section_takes_at_most_a_third() {
        assert_eq!(hidden_section_height(0, 30), 0);
        assert_eq!(hidden_section_height(2, 30), 3);
        assert_eq!(hidden_section_height(20, 30), 10);
        assert_eq!(hidden_section_height(3, 4), 0);
    }
}
