//! collie: `herdr chat`, Befehle rund um Agent-Chats.
//!
//! `herdr chat search` durchsucht alle Claude-Transcripts mit `fzf` und öffnet
//! den gewählten Chat: ein laufendes Pane wird fokussiert, sonst startet ein
//! neuer Space mit `claude --resume`. Siehe docs/specs/chat-workflow.md.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::api::schema::{
    Method, PaneListParams, PaneSendInputParams, PaneTarget, Request, WorkspaceCreateParams,
};

/// Höchstlänge einer Trefferzeile in Zeichen.
const LINE_CHARS: usize = 300;
/// Nachrichten vor dem Treffer, die die Vorschau zeigt.
const PREVIEW_BEFORE: usize = 2;
/// Nachrichten nach dem Treffer, die die Vorschau zeigt.
const PREVIEW_AFTER: usize = 8;
/// Version des Cache-Formats. Eine neue Version verwirft alte Einträge.
const CACHE_VERSION: u32 = 2;

pub(super) fn run_chat_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("search") => chat_search(&args[1..]),
        Some("preview") => chat_preview(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_chat_help();
            Ok(0)
        }
        _ => {
            print_chat_help();
            Ok(2)
        }
    }
}

fn print_chat_help() {
    eprintln!("herdr chat commands:");
    eprintln!("  herdr chat search [--print]");
    eprintln!("  herdr chat preview <transcript> <line>");
}

/// Eine Nachricht eines Transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatMessage {
    /// Zeilennummer im Transcript, ab 1.
    line: usize,
    user: bool,
    /// Zeitstempel wie im Transcript, ISO 8601.
    timestamp: String,
    cwd: String,
    text: String,
}

/// Liest die sichtbaren Nachrichten eines Transcripts.
fn read_messages(path: &Path) -> std::io::Result<Vec<ChatMessage>> {
    let file = std::fs::File::open(path)?;
    let mut messages = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let Ok(line) = line else {
            continue;
        };
        if let Some(message) = parse_message(index + 1, &line) {
            messages.push(message);
        }
    }
    Ok(messages)
}

fn parse_message(line: usize, raw: &str) -> Option<ChatMessage> {
    // Die meisten Zeilen sind Tool-Ergebnisse. Ein schneller Vorfilter spart
    // das JSON-Parsen für sie.
    if !raw.contains("\"type\":\"user\"") && !raw.contains("\"type\":\"assistant\"") {
        return None;
    }
    let entry: serde_json::Value = serde_json::from_str(raw).ok()?;
    let user = match entry.get("type")?.as_str()? {
        "user" => true,
        "assistant" => false,
        _ => return None,
    };
    if entry
        .get("isSidechain")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        || entry.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = entry.get("message")?.get("content")?;
    let text = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    // Befehle, System-Erinnerungen und Hook-Ausgaben beginnen mit einem Tag.
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    Some(ChatMessage {
        line,
        user,
        timestamp: entry
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        cwd: entry
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        text: text.to_owned(),
    })
}

/// Eine Cache-Zeile: `<zeile>\t<zeitstempel>\t<projekt>  <wer> <text>`.
/// Der Zeitstempel bleibt in UTC, damit der Cache nicht von der Zeitzone
/// abhängt.
fn cache_line(message: &ChatMessage) -> String {
    let project = Path::new(&message.cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let who = if message.user { "" } else { "󰚩" };
    let mut text = message
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((cut, _)) = text.char_indices().nth(LINE_CHARS) {
        text.truncate(cut);
    }
    format!(
        "{}\t{}\t\x1b[36m{project}\x1b[0m  {who} {text}",
        message.line, message.timestamp
    )
}

/// Trefferzeile für fzf: `<pfad>\t<zeile>\t<ortszeit>  <projekt>  <wer> <text>`.
fn search_line(path: &Path, cache_line: &str, offset_secs: i64) -> Option<String> {
    let mut fields = cache_line.splitn(3, '\t');
    let (line, stamp, rest) = (fields.next()?, fields.next()?, fields.next()?);
    let when = local_stamp(stamp, offset_secs).unwrap_or_default();
    Some(format!(
        "{}\t{line}\t\x1b[2m{when}\x1b[0m  {rest}",
        path.display()
    ))
}

/// Abstand der Ortszeit zu UTC in Sekunden, jetzt.
fn local_offset_secs() -> i64 {
    let Some(local) = crate::platform::local_datetime() else {
        return 0;
    };
    let utc = time::OffsetDateTime::now_utc();
    let utc = time::PrimitiveDateTime::new(utc.date(), utc.time());
    let seconds = (local - utc).whole_seconds();
    // Auf Viertelstunden runden; die beiden Uhren laufen nicht exakt gleich.
    (seconds as f64 / 900.0).round() as i64 * 900
}

/// `2026-10-12T17:29:03.123Z` als Ortszeit `2026-10-12 19:29`.
fn local_stamp(stamp: &str, offset_secs: i64) -> Option<String> {
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<u32>().ok();
    let month = time::Month::try_from(u8::try_from(number(5..7)?).ok()?).ok()?;
    let date = time::Date::from_calendar_date(
        i32::try_from(number(0..4)?).ok()?,
        month,
        u8::try_from(number(8..10)?).ok()?,
    )
    .ok()?;
    let clock = time::Time::from_hms(
        u8::try_from(number(11..13)?).ok()?,
        u8::try_from(number(14..16)?).ok()?,
        0,
    )
    .ok()?;
    let local = time::PrimitiveDateTime::new(date, clock)
        .checked_add(time::Duration::seconds(offset_secs))?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        local.year(),
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute()
    ))
}

fn projects_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|home| home.join(".claude")))?;
    Some(base.join("projects"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|home| home.join(".cache")))?;
    Some(base.join("collie").join("chat-search"))
}

/// Alle Transcripts, neueste zuerst.
fn transcripts() -> Vec<PathBuf> {
    let Some(projects) = projects_dir() else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    for dir in dirs.flatten() {
        let Ok(entries) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let modified = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                files.push((modified, path));
            }
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.into_iter().map(|(_, path)| path).collect()
}

/// Kennung eines Transcripts für den Cache: Größe und mtime.
fn transcript_stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("v{CACHE_VERSION} {} {modified}", meta.len()))
}

fn cache_file(cache: &Path, transcript: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    transcript.hash(&mut hasher);
    cache.join(format!("{:016x}.txt", hasher.finish()))
}

/// Trefferzeilen eines Transcripts, neueste zuerst, aus dem Cache oder frisch.
fn transcript_lines(cache: Option<&Path>, transcript: &Path) -> Vec<String> {
    let stamp = transcript_stamp(transcript);
    let cached = cache.map(|cache| cache_file(cache, transcript));
    if let (Some(stamp), Some(cached)) = (&stamp, &cached) {
        if let Ok(content) = std::fs::read_to_string(cached) {
            if let Some((header, body)) = content.split_once('\n') {
                if header == stamp {
                    return body.lines().map(str::to_owned).collect();
                }
            }
        }
    }
    let Ok(messages) = read_messages(transcript) else {
        return Vec::new();
    };
    let lines = messages.iter().rev().map(cache_line).collect::<Vec<_>>();
    if let (Some(stamp), Some(cached)) = (stamp, cached) {
        let mut content = stamp;
        for line in &lines {
            content.push('\n');
            content.push_str(line);
        }
        if let Some(dir) = cached.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(cached, content);
    }
    lines
}

fn chat_search(args: &[String]) -> std::io::Result<i32> {
    let print_only = match args {
        [] => false,
        [flag] if flag == "--print" => true,
        _ => {
            eprintln!("usage: herdr chat search [--print]");
            return Ok(2);
        }
    };
    let cache = cache_dir();
    let files = transcripts();
    let offset = local_offset_secs();
    if print_only {
        let mut stdout = std::io::stdout().lock();
        for file in &files {
            for line in transcript_lines(cache.as_deref(), file) {
                if let Some(line) = search_line(file, &line, offset) {
                    if writeln!(stdout, "{line}").is_err() {
                        return Ok(0);
                    }
                }
            }
        }
        return Ok(0);
    }

    let exe = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "herdr".to_owned());
    let mut fzf = match Command::new("fzf")
        .args([
            "--ansi",
            "--delimiter=\t",
            "--with-nth=3..",
            "--tiebreak=index",
            "--layout=reverse",
            "--info=inline-right",
            "--prompt=  ",
            "--pointer=▌",
            "--gutter= ",
            "--border=rounded",
            "--border-label= chats ",
            "--preview-window=right,55%,wrap,border-left",
            "--bind=ctrl-/:toggle-preview",
        ])
        .arg(format!(
            "--preview={} chat preview {{1}} {{2}}",
            shell_quote(&exe)
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("herdr chat search needs fzf on PATH");
            return Ok(1);
        }
        Err(err) => return Err(err),
    };

    // Die Zeilen fließen, während fzf schon läuft; die Liste füllt sich von
    // oben mit den neuesten Chats.
    let Some(mut stdin) = fzf.stdin.take() else {
        return Err(std::io::Error::other("fzf has no stdin"));
    };
    let feeder = std::thread::spawn(move || {
        for file in &files {
            for line in transcript_lines(cache.as_deref(), file) {
                let Some(line) = search_line(file, &line, offset) else {
                    continue;
                };
                if writeln!(stdin, "{line}").is_err() {
                    return;
                }
            }
        }
    });

    let output = fzf.wait_with_output()?;
    let _ = feeder.join();
    let selected = String::from_utf8_lossy(&output.stdout);
    let Some(selected) = selected.lines().next() else {
        return Ok(0);
    };
    let mut fields = selected.split('\t');
    let (Some(path), Some(line)) = (fields.next(), fields.next()) else {
        return Ok(0);
    };
    let line = line.parse::<usize>().unwrap_or(0);
    open_chat(Path::new(path), line)
}

/// Fokussiert den Chat zu einem Transcript oder startet ihn neu.
fn open_chat(transcript: &Path, line: usize) -> std::io::Result<i32> {
    let Some(session) = transcript
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
    else {
        return Ok(1);
    };
    if let Some(pane_id) = pane_with_session(&session)? {
        return super::send_ok_request(Method::PaneFocus(PaneTarget { pane_id }));
    }

    let messages = read_messages(transcript)?;
    let message = messages
        .iter()
        .find(|message| message.line == line)
        .or(messages.last());
    let cwd = message
        .map(|message| message.cwd.clone())
        .filter(|cwd| Path::new(cwd).is_dir())
        .or_else(|| dirs_home().map(|home| home.display().to_string()));
    let response = super::send_request(&Request {
        id: "cli:chat:create".into(),
        method: Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd,
            focus: true,
            label: None,
            env: std::collections::HashMap::new(),
        }),
    })?;
    if response.get("error").is_some() {
        return super::print_response(&response);
    }
    let Some(pane_id) = response
        .pointer("/result/root_pane/pane_id")
        .or_else(|| response.pointer("/result/pane/pane_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        eprintln!("workspace.create returned no pane");
        return Ok(1);
    };
    super::send_ok_request(Method::PaneSendInput(PaneSendInputParams {
        pane_id,
        text: format!("claude --resume {}", shell_quote(&session)),
        keys: vec!["Enter".into()],
    }))
}

/// Pane, in dem die Claude-Session gerade läuft.
fn pane_with_session(session: &str) -> std::io::Result<Option<String>> {
    let response = super::send_request(&Request {
        id: "cli:chat:panes".into(),
        method: Method::PaneList(PaneListParams { workspace_id: None }),
    })?;
    let panes = response
        .pointer("/result/panes")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(panes.iter().find_map(|pane| {
        (pane
            .pointer("/agent_session/value")
            .and_then(serde_json::Value::as_str)
            == Some(session))
        .then(|| pane.get("pane_id")?.as_str().map(str::to_owned))
        .flatten()
    }))
}

fn chat_preview(args: &[String]) -> std::io::Result<i32> {
    let [path, line] = args else {
        eprintln!("usage: herdr chat preview <transcript> <line>");
        return Ok(2);
    };
    let line = line.parse::<usize>().unwrap_or(0);
    let path = Path::new(path);
    let messages = read_messages(path)?;
    let Some(index) = messages.iter().position(|message| message.line == line) else {
        return Ok(0);
    };
    let mut stdout = std::io::stdout().lock();
    let offset = local_offset_secs();
    let selected = &messages[index];
    let session = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    writeln!(
        stdout,
        "\x1b[36m{}\x1b[0m  \x1b[2m{session}\x1b[0m\n",
        selected.cwd
    )?;
    let start = index.saturating_sub(PREVIEW_BEFORE);
    let end = (index + PREVIEW_AFTER + 1).min(messages.len());
    for (position, message) in messages[start..end].iter().enumerate() {
        let who = if message.user { " du" } else { "󰚩 agent" };
        let when = local_stamp(&message.timestamp, offset)
            .and_then(|stamp| stamp.get(11..).map(str::to_owned))
            .unwrap_or_default();
        let marker = if start + position == index {
            "\x1b[1;33m"
        } else {
            "\x1b[1m"
        };
        writeln!(stdout, "{marker}{who}\x1b[0m \x1b[2m{when}\x1b[0m")?;
        writeln!(stdout, "{}\n", message.text)?;
    }
    Ok(0)
}

/// POSIX-Quoting für einen Shell-Befehl.
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_keeps_visible_user_and_agent_text() {
        let user = r#"{"type":"user","timestamp":"2026-10-12T19:15:00Z","cwd":"/p/collie","message":{"content":"Sidebar umbauen"}}"#;
        let message = parse_message(3, user).unwrap();
        assert!(message.user);
        assert_eq!(message.text, "Sidebar umbauen");
        assert_eq!(message.line, 3);

        let agent = r#"{"type":"assistant","cwd":"/p","message":{"content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"Fertig."}]}}"#;
        assert_eq!(parse_message(4, agent).unwrap().text, "Fertig.");
    }

    #[test]
    fn parse_message_skips_tool_results_meta_and_tags() {
        for raw in [
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"x"}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"x"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"summary","summary":"x"}"#,
        ] {
            assert_eq!(parse_message(1, raw), None, "{raw}");
        }
    }

    #[test]
    fn local_stamp_applies_the_offset() {
        assert_eq!(
            local_stamp("2026-10-12T23:29:03.123Z", 7200).as_deref(),
            Some("2026-10-13 01:29")
        );
        assert_eq!(local_stamp("kaputt", 0), None);
    }

    #[test]
    fn search_line_starts_with_the_transcript_line_and_truncates() {
        let message = ChatMessage {
            line: 7,
            user: false,
            timestamp: "2026-10-12T19:15:42Z".into(),
            cwd: "/home/u/projects/collie".into(),
            text: format!("a\n{}", "x".repeat(LINE_CHARS * 2)),
        };
        let cached = cache_line(&message);
        assert!(cached.starts_with("7\t2026-10-12T19:15:42Z\t"));
        let line = search_line(Path::new("/t/s.jsonl"), &cached, 0).unwrap();
        assert!(line.starts_with("/t/s.jsonl\t7\t"));
        assert!(line.contains("2026-10-12 19:15"));
        assert!(line.contains("collie"));
        assert!(!line.contains('\n'));
        assert!(line.len() < LINE_CHARS + 120);
    }

    #[test]
    fn shell_quote_quotes_only_when_needed() {
        assert_eq!(shell_quote("abc-123"), "abc-123");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
