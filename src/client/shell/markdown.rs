//! collie: Markdown als Terminal-Zeilen für die Vorschau der Nutzer-Inbox.
//! Agents antworten in Markdown; Überschriften, Hervorhebungen, Listen,
//! Zitate, Code und Tabellen werden hier gesetzt statt roh gezeigt.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};

use super::render::display_width;
use super::Palette;
use crate::ui::truncate_end;

/// Eine Zeile aus gestylten Stücken.
pub(super) type MdLine = Vec<(String, Style)>;

/// Setzt `text` auf `width` Spalten.
pub(super) fn render_markdown(
    text: &str,
    width: usize,
    palette: &Palette,
    base: Style,
) -> Vec<MdLine> {
    let mut writer = Writer {
        width: width.max(8),
        palette,
        base,
        lines: Vec::new(),
        inline: Vec::new(),
        styles: Vec::new(),
        lists: Vec::new(),
        item_prefix: None,
        quote: 0,
        heading: None,
        code: None,
        table: None,
        link: None,
    };
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(text, options) {
        writer.event(event);
    }
    writer.flush_inline();
    while writer.lines.last().is_some_and(|line| line.is_empty()) {
        writer.lines.pop();
    }
    writer.lines
}

struct Table {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<MdLine>>,
    header_rows: usize,
    in_head: bool,
}

struct Writer<'a> {
    width: usize,
    palette: &'a Palette,
    base: Style,
    lines: Vec<MdLine>,
    /// Text des laufenden Absatzes, noch nicht umbrochen.
    inline: MdLine,
    styles: Vec<Style>,
    /// Offene Listen: `Some(n)` nummeriert ab `n`.
    lists: Vec<Option<u64>>,
    /// Aufzählungszeichen des Punkts, der gerade beginnt.
    item_prefix: Option<String>,
    quote: usize,
    heading: Option<HeadingLevel>,
    code: Option<Vec<String>>,
    table: Option<Table>,
    link: Option<String>,
}

impl Writer<'_> {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or(self.base)
    }

    fn push_style(&mut self, update: impl FnOnce(Style) -> Style) {
        let style = update(self.style());
        self.styles.push(style);
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|line| !line.is_empty()) {
            self.lines.push(Vec::new());
        }
    }

    /// Einzug vor jeder Zeile: Zitatbalken und Listenebenen.
    fn indent(&self) -> MdLine {
        let mut prefix = Vec::new();
        for _ in 0..self.quote {
            prefix.push(("│ ".to_owned(), self.base.fg(self.palette.overlay0)));
        }
        let depth = self.lists.len();
        if depth > 0 {
            prefix.push(("  ".repeat(depth - 1), self.base));
        }
        prefix
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(code) = self.code.as_mut() {
            match event {
                Event::Text(text) => {
                    for line in text.lines() {
                        code.push(line.to_owned());
                    }
                }
                Event::End(TagEnd::CodeBlock) => self.end_code_block(),
                _ => {}
            }
            return;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                let style = self.style().fg(self.palette.peach);
                self.inline.push((code.into_string(), style));
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush_inline(),
            Event::Rule => {
                self.flush_inline();
                self.blank();
                let width = self.width.saturating_sub(display_width_of(&self.indent()));
                let mut line = self.indent();
                line.push(("─".repeat(width), self.base.fg(self.palette.surface1)));
                self.lines.push(line);
                self.blank();
            }
            Event::TaskListMarker(done) => {
                let (mark, color) = if done {
                    ("☑ ", self.palette.green)
                } else {
                    ("☐ ", self.palette.overlay1)
                };
                self.inline.push((mark.to_owned(), self.base.fg(color)));
            }
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        let style = self.style();
        self.inline.push((text.to_owned(), style));
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.lists.is_empty() && self.table.is_none() {
                    self.blank();
                }
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.blank();
                self.heading = Some(level);
                let color = match level {
                    HeadingLevel::H1 | HeadingLevel::H2 => self.palette.accent,
                    _ => self.palette.mauve,
                };
                let mut style = self.base.fg(color).add_modifier(Modifier::BOLD);
                if level == HeadingLevel::H1 {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.blank();
                self.quote += 1;
                self.push_style(|style| style.add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.blank();
                if let CodeBlockKind::Fenced(lang) = kind {
                    if !lang.is_empty() {
                        let mut line = self.indent();
                        line.push((
                            format!("  {lang}"),
                            self.base
                                .fg(self.palette.overlay0)
                                .add_modifier(Modifier::ITALIC),
                        ));
                        self.lines.push(line);
                    }
                }
                self.code = Some(Vec::new());
            }
            Tag::List(start) => {
                self.flush_inline();
                if self.lists.is_empty() {
                    self.blank();
                }
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush_inline();
                let prefix = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let prefix = format!("{number}. ");
                        *number += 1;
                        prefix
                    }
                    _ => "• ".to_owned(),
                };
                self.item_prefix = Some(prefix);
            }
            Tag::Emphasis => self.push_style(|style| style.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|style| style.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(|style| style.add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.into_string());
                let color = self.palette.blue;
                self.push_style(|style| style.fg(color).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Table(alignments) => {
                self.flush_inline();
                self.blank();
                self.table = Some(Table {
                    alignments,
                    rows: Vec::new(),
                    header_rows: 0,
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.in_head = true;
                    table.rows.push(Vec::new());
                }
                self.push_style(|style| style.add_modifier(Modifier::BOLD));
            }
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.inline.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.table.is_none() {
                    self.flush_inline();
                }
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.heading = None;
                self.styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.quote = self.quote.saturating_sub(1);
                self.styles.pop();
                self.blank();
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush_inline(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                // Nackte URLs stehen schon als Text da.
                if let Some(url) = self.link.take() {
                    let shown = self.inline.last().is_some_and(|(text, _)| text == &url);
                    if !shown && !url.is_empty() && !url.starts_with('#') {
                        let style = self.base.fg(self.palette.overlay0);
                        self.inline.push((format!(" ({url})"), style));
                    }
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(row) = self.table.as_mut().and_then(|table| table.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.in_head = false;
                    table.header_rows = table.rows.len();
                }
                self.styles.pop();
            }
            TagEnd::Table => self.end_table(),
            _ => {}
        }
    }

    fn end_code_block(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let style = self.base.fg(self.palette.green);
        let room = self
            .width
            .saturating_sub(display_width_of(&self.indent()) + 2);
        for text in code {
            let mut line = self.indent();
            line.push(("  ".to_owned(), self.base));
            line.push((truncate_end(&text.replace('\t', "    "), room), style));
            self.lines.push(line);
        }
        self.blank();
    }

    /// Bricht den laufenden Absatz um und hängt ihn an.
    fn flush_inline(&mut self) {
        let prefix = self.item_prefix.take();
        if self.inline.is_empty() {
            if let Some(prefix) = prefix {
                // Leerer Punkt, etwa vor einer verschachtelten Liste.
                let mut line = self.indent();
                line.push((prefix, self.base.fg(self.palette.overlay1)));
                self.lines.push(line);
            }
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        let indent = self.indent();
        let mut first = indent.clone();
        let mut rest = indent;
        if let Some(prefix) = prefix {
            let width = display_width(&prefix) as usize;
            first.push((prefix, self.base.fg(self.palette.overlay1)));
            rest.push((" ".repeat(width), self.base));
        } else if !self.lists.is_empty() {
            // Folgeabsatz in einem Listenpunkt.
            rest.push(("  ".to_owned(), self.base));
            first = rest.clone();
        }
        let lines = wrap_spans(&spans, &first, &rest, self.width);
        self.lines.extend(lines);
    }

    fn end_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let indent = self.indent();
        let room = self
            .width
            .saturating_sub(display_width_of(&indent))
            .saturating_sub(3 * (columns - 1));
        let mut widths = vec![0usize; columns];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(display_width_of(cell));
            }
        }
        shrink_columns(&mut widths, room);
        let separator = self.base.fg(self.palette.surface1);
        for (index, row) in table.rows.iter().enumerate() {
            let mut line = indent.clone();
            for (column, width) in widths.iter().enumerate() {
                if column > 0 {
                    line.push((" │ ".to_owned(), separator));
                }
                let cell = row.get(column).cloned().unwrap_or_default();
                let alignment = table
                    .alignments
                    .get(column)
                    .copied()
                    .unwrap_or(Alignment::None);
                line.extend(fit_cell(&cell, *width, alignment, self.base));
            }
            self.lines.push(line);
            if index + 1 == table.header_rows {
                let mut rule = indent.clone();
                let text = widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                rule.push((text, separator));
                self.lines.push(rule);
            }
        }
        self.blank();
    }
}

fn display_width_of(spans: &[(String, Style)]) -> usize {
    spans
        .iter()
        .map(|(text, _)| display_width(text) as usize)
        .sum()
}

/// Verkleinert die breitesten Spalten, bis die Tabelle in `room` passt.
fn shrink_columns(widths: &mut [usize], room: usize) {
    while widths.iter().sum::<usize>() > room {
        let Some(widest) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, width)| **width)
            .map(|(index, _)| index)
        else {
            return;
        };
        if widths[widest] <= 3 {
            return;
        }
        widths[widest] -= 1;
    }
}

/// Zelle auf genau `width` Spalten: gekürzt oder aufgefüllt.
fn fit_cell(cell: &[(String, Style)], width: usize, alignment: Alignment, base: Style) -> MdLine {
    let mut out = Vec::new();
    let mut used = 0;
    let total = display_width_of(cell);
    for (text, style) in cell {
        let room = width.saturating_sub(used);
        if room == 0 {
            break;
        }
        let text = if used + display_width(text) as usize > width
            || total > width && used + display_width(text) as usize == width
        {
            truncate_end(text, room)
        } else {
            text.clone()
        };
        used += display_width(&text) as usize;
        out.push((text, *style));
    }
    let pad = width.saturating_sub(used);
    let (left, right) = match alignment {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    if left > 0 {
        out.insert(0, (" ".repeat(left), base));
    }
    if right > 0 {
        out.push((" ".repeat(right), base));
    }
    out
}

/// Bricht gestylte Stücke an Leerzeichen auf `width` um. Die erste Zeile
/// beginnt mit `first`, jede weitere mit `rest`.
fn wrap_spans(
    spans: &[(String, Style)],
    first: &MdLine,
    rest: &MdLine,
    width: usize,
) -> Vec<MdLine> {
    // Wörter samt Stil; `true` heißt, davor stand ein Leerzeichen.
    let mut words: Vec<(bool, String, Style)> = Vec::new();
    let mut space = false;
    for (text, style) in spans {
        let mut current = String::new();
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !current.is_empty() {
                    words.push((space, std::mem::take(&mut current), *style));
                }
                space = true;
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            words.push((space, current, *style));
            space = false;
        }
    }
    let mut lines = Vec::new();
    let mut line = first.clone();
    let mut used = display_width_of(first);
    let mut empty = true;
    for (space_before, word, style) in words {
        let word_width = display_width(&word) as usize;
        let gap = usize::from(space_before && !empty);
        if !empty && used + gap + word_width > width {
            lines.push(std::mem::replace(&mut line, rest.clone()));
            used = display_width_of(rest);
            empty = true;
        }
        if gap == 1 && !empty {
            line.push((" ".to_owned(), style));
            used += 1;
        }
        let room = width.saturating_sub(used).max(1);
        let word = if word_width > room {
            truncate_end(&word, room)
        } else {
            word
        };
        used += display_width(&word) as usize;
        line.push((word, style));
        empty = false;
    }
    if !empty {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str, width: usize) -> Vec<String> {
        let config =
            super::super::ClientShellConfig::from_config(&crate::config::Config::default());
        render_markdown(text, width, &config.palette, Style::default())
            .into_iter()
            .map(|line| {
                line.into_iter()
                    .map(|(text, _)| text)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn headings_and_emphasis_lose_their_markup() {
        let lines = plain("## Stand\n\nDas ist **fett** und `code`.", 40);
        assert_eq!(lines, vec!["Stand", "", "Das ist fett und code."]);
    }

    #[test]
    fn lists_get_bullets_and_hanging_indent() {
        let lines = plain("- eins zwei drei vier\n- fünf\n\n1. a\n2. b", 14);
        assert_eq!(
            lines,
            vec!["• eins zwei", "  drei vier", "• fünf", "", "1. a", "2. b"]
        );
    }

    #[test]
    fn tables_are_aligned_with_separators() {
        let lines = plain(
            "| Child | Status |\n|---|---|\n| Login | fertig |\n| API | läuft |",
            40,
        );
        assert_eq!(
            lines,
            vec![
                "Child │ Status",
                "──────┼───────",
                "Login │ fertig",
                "API   │ läuft",
            ]
        );
    }

    #[test]
    fn wide_tables_shrink_to_the_width() {
        let lines = plain(
            "| a | b |\n|---|---|\n| sehr langer Zelleninhalt | noch ein langer Text |",
            24,
        );
        assert!(
            lines.iter().all(|line| display_width(line) as usize <= 24),
            "{lines:?}"
        );
    }

    #[test]
    fn code_blocks_and_quotes_keep_their_shape() {
        let lines = plain("```sh\ncargo test\n```\n\n> Zitat", 30);
        assert_eq!(lines, vec!["  sh", "  cargo test", "", "│ Zitat"]);
    }

    #[test]
    fn links_show_their_target() {
        let lines = plain("[Spec](https://x.y/spec) und https://x.y", 60);
        assert_eq!(lines, vec!["Spec (https://x.y/spec) und https://x.y"]);
    }
}
