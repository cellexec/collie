//! collie: Schmale Chat-Leiste statt der Linie, solange das Terminal breit
//! genug ist (Laptop, Desktop). Sie zeigt je Chat Status und Repo-Kürzel,
//! oben die wartenden, unten abgesetzt die laufenden und gesnoozten Chats.
//! Auf schmalen Terminals bleibt die Linie von 1 Spalte.
//!
//! Gezeichnet wird nur aus Snapshot-Feldern, wie bei den Karten.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::chat_visuals::{heat_color, repo_color, repo_initials};
use super::chats::{chat_symbol, overlay_chat_rows, spinner_frame, ChatRow};
use super::render::{display_width, put_text};
use super::*;
use crate::api::schema::AgentStatus;

/// Ab dieser Terminalbreite steht die Leiste statt der Linie.
pub(super) const RAIL_MIN_COLS: u16 = 110;
/// Breite der Leiste: Streifen, Status, Lücke, Kürzel-Chip, Rand.
pub(super) const RAIL_WIDTH: u16 = 8;

const ICON_CHATS: &str = "󰭹";
const ICON_SNOOZE: &str = "󰒲";

/// Breite der Chat-Spalte links: Leiste auf breiten Terminals, sonst Linie.
pub(super) fn chat_column_width(cols: u16) -> u16 {
    if cols >= RAIL_MIN_COLS {
        RAIL_WIDTH
    } else {
        1
    }
}

/// Kürzel eines Chats: aus dem Repo, sonst aus dem Titel.
fn chat_initials(row: &ChatRow) -> String {
    let initials = row
        .repo
        .as_deref()
        .map(repo_initials)
        .filter(|initials| !initials.is_empty())
        .unwrap_or_else(|| repo_initials(&row.label));
    if initials.is_empty() {
        "··".to_owned()
    } else {
        format!("{initials:<2}")
    }
}

/// Höhe des unteren Abschnitts: Trennlinie plus eine Zeile je Chat, höchstens
/// ein Drittel der Fläche.
fn hidden_height(rows: usize, available: u16) -> u16 {
    if rows == 0 || available < 6 {
        return 0;
    }
    let wanted = u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(1);
    wanted.min((available / 3).max(2))
}

/// Zeichnet die Leiste und meldet sie als Linie, dazu jede Chat-Zeile als
/// Treffer.
pub(super) fn render_chat_rail(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    bright: bool,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let now = crate::workspace::unix_millis_now();
    hits.sidebar_line = area;
    if area.width < 3 || area.height < 3 {
        return;
    }
    buffer.set_style(area, Style::default().bg(palette.sidebar_bg));

    // Rechter Rand in der Farbe der bisherigen Linie: rot, Akzent oder gedimmt.
    let border = if bright {
        palette.text
    } else {
        super::chat_overlay::line_color(snapshot, now, palette)
    };
    let border_x = area.right().saturating_sub(1);
    for y in area.y..area.bottom() {
        put_text(buffer, border_x, y, 1, "▕", Style::default().fg(border));
    }
    let inner = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);

    let (waiting, hidden) = overlay_chat_rows(snapshot, config.chat_newest_first, "", now);
    let hidden = hidden
        .into_iter()
        .filter(|row| row.status == AgentStatus::Working || row.snoozed(now))
        .collect::<Vec<_>>();

    // Kopf: Icon und Zahl der wartenden Chats.
    let count = waiting.len();
    let header = if count == 0 {
        format!(" {ICON_CHATS}")
    } else {
        format!(" {ICON_CHATS} {count}")
    };
    let header_style = if count == 0 {
        Style::default().fg(palette.overlay0)
    } else {
        Style::default().fg(border).add_modifier(Modifier::BOLD)
    };
    put_text(buffer, inner.x, inner.y, inner.width, &header, header_style);

    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        inner.width,
        inner.height.saturating_sub(2),
    );
    let bottom = hidden_height(hidden.len(), body.height);
    let main = Rect::new(
        body.x,
        body.y,
        body.width,
        body.height
            .saturating_sub(bottom)
            .saturating_sub(u16::from(bottom > 0)),
    );
    render_waiting(buffer, main, &waiting, now, palette, hits);
    if bottom > 0 {
        let section = Rect::new(
            body.x,
            body.bottom().saturating_sub(bottom),
            body.width,
            bottom,
        );
        render_hidden(buffer, section, &hidden, now, palette, hits);
    }
}

/// Wartende Chats, eine Zeile je Chat. Mit genug Platz steht eine Leerzeile
/// dazwischen. Was nicht passt, zählt die letzte Zeile als `+N`.
fn render_waiting(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[ChatRow],
    now: u64,
    palette: &Palette,
    hits: &mut ShellHitMap,
) {
    if area.height == 0 || rows.is_empty() {
        return;
    }
    let height = usize::from(area.height);
    let step = if rows.len() * 2 - 1 <= height { 2 } else { 1 };
    let fits = height.div_ceil(step);
    let shown = if rows.len() > fits {
        fits.saturating_sub(1)
    } else {
        rows.len()
    };
    for (index, row) in rows.iter().take(shown).enumerate() {
        let rect = Rect::new(area.x, area.y + (index * step) as u16, area.width, 1);
        render_waiting_row(buffer, rect, row, now, palette);
        hits.agents.push((rect, row.pane_id.clone()));
    }
    if shown < rows.len() {
        let more = format!(" +{}", rows.len() - shown);
        put_text(
            buffer,
            area.x,
            area.y + (shown * step) as u16,
            area.width,
            &more,
            Style::default().fg(palette.overlay0),
        );
    }
}

fn render_waiting_row(buffer: &mut Buffer, rect: Rect, row: &ChatRow, now: u64, palette: &Palette) {
    let base = if row.focused {
        Style::default().bg(palette.surface0)
    } else {
        Style::default().bg(palette.sidebar_bg)
    };
    buffer.set_style(rect, base);
    // Streifen links: Akzent für den offenen Chat, sonst Wärme nach Wartezeit.
    let stripe = if row.focused {
        palette.accent
    } else {
        heat_color(row.attention_since, now, palette)
    };
    put_text(buffer, rect.x, rect.y, 1, "▌", base.fg(stripe));
    if let Some((symbol, style)) = chat_symbol(row, now, palette) {
        let symbol = if row.unread || symbol != "●" {
            symbol
        } else {
            "○"
        };
        put_text(
            buffer,
            rect.x.saturating_add(1),
            rect.y,
            1,
            symbol,
            style.bg(base.bg.unwrap_or(palette.sidebar_bg)),
        );
    }
    put_chip(buffer, rect, row, palette, true);
}

/// Laufende und gesnoozte Chats unter einer Trennlinie.
fn render_hidden(
    buffer: &mut Buffer,
    section: Rect,
    rows: &[ChatRow],
    now: u64,
    palette: &Palette,
    hits: &mut ShellHitMap,
) {
    for x in section.x..section.right() {
        put_text(
            buffer,
            x,
            section.y,
            1,
            "─",
            Style::default().fg(palette.surface1),
        );
    }
    let visible = usize::from(section.height.saturating_sub(1));
    for (offset, row) in rows.iter().take(visible).enumerate() {
        let rect = Rect::new(section.x, section.y + 1 + offset as u16, section.width, 1);
        let (symbol, style) = if row.snoozed(now) {
            (ICON_SNOOZE, Style::default().fg(palette.overlay0))
        } else {
            (
                spinner_frame(now),
                Style::default().fg(status_color(AgentStatus::Working, palette)),
            )
        };
        put_text(buffer, rect.x.saturating_add(1), rect.y, 1, symbol, style);
        put_chip(buffer, rect, row, palette, false);
        hits.agents.push((rect, row.pane_id.clone()));
    }
}

/// Repo-Kürzel ab Spalte 3. Wartende Chats bekommen einen farbigen Chip,
/// laufende nur farbigen Text, damit oben ins Auge fällt, was wartet.
fn put_chip(buffer: &mut Buffer, rect: Rect, row: &ChatRow, palette: &Palette, filled: bool) {
    let initials = chat_initials(row);
    let hue = row
        .repo
        .as_deref()
        .map_or(palette.overlay1, |repo| repo_color(repo, palette));
    let x = rect.x.saturating_add(3);
    let width = rect.right().saturating_sub(x);
    if filled {
        let chip = format!(" {initials} ");
        put_text(
            buffer,
            x,
            rect.y,
            display_width(&chip).min(width),
            &chip,
            Style::default()
                .fg(palette.panel_bg)
                .bg(hue)
                .add_modifier(Modifier::BOLD),
        );
    } else {
        put_text(
            buffer,
            x.saturating_add(1),
            rect.y,
            width.saturating_sub(1),
            &initials,
            Style::default().fg(hue),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_needs_a_wide_terminal() {
        assert_eq!(chat_column_width(100), 1);
        assert_eq!(chat_column_width(RAIL_MIN_COLS), RAIL_WIDTH);
        assert_eq!(chat_column_width(240), RAIL_WIDTH);
    }

    #[test]
    fn hidden_section_takes_at_most_a_third() {
        assert_eq!(hidden_height(0, 30), 0);
        assert_eq!(hidden_height(2, 30), 3);
        assert_eq!(hidden_height(20, 30), 10);
    }
}
