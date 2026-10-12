//! collie: Chat-Hook für Claude Code.
//!
//! `collie integration install claude` legt neben dem herdr-Hook den Chat-Hook
//! aus `scripts/collie/claude-chat-hook.sh` ab und trägt ihn für `Stop` und
//! `Notification` in `settings.json` ein. Er meldet die letzte Nachricht und
//! den Blocked-Grund per `pane.report_chat`, damit die Chat-Karten gefüllt sind.
//! Der herdr-Teil der Integration bleibt unverändert, damit Upstream-Merges
//! sauber bleiben.

use std::io;
use std::path::{Path, PathBuf};

use jsonc_parser::cst::{CstInputValue, CstNode, CstRootNode};
use jsonc_parser::{json, ParseOptions};
use serde_json::Value;

use super::command::hook_command;
use super::config_file::write_config;
use super::env::claude_dir;
use super::file_ops::{make_executable, remove_file_if_exists};

const CHAT_HOOK_INSTALL_NAME: &str = "collie-chat-hook.sh";
const CHAT_HOOK_ASSET: &str = include_str!("../../scripts/collie/claude-chat-hook.sh");
const CHAT_HOOK_TIMEOUT: u64 = 5;
/// Hook-Event in Claude Code und das Argument, mit dem der Hook es auswertet.
const CHAT_HOOK_EVENTS: &[(&str, &str)] = &[("Stop", "stop"), ("Notification", "notification")];
/// Dateiname, unter dem der Hook vor der Integration von Hand eingetragen wurde.
const LEGACY_HOOK_NAME: &str = "claude-chat-hook.sh";

/// Schreibt den Hook nach `hooks/` und trägt ihn in `settings.json` ein.
/// Läuft nach `install_claude`, das Verzeichnis und Settings schon geprüft hat.
/// Hand-Einträge eines älteren Setups, die auf `claude-chat-hook.sh` zeigen,
/// ersetzt es.
pub(crate) fn install_for_claude() -> io::Result<PathBuf> {
    let dir = claude_dir()?;
    let hook_path = dir.join("hooks").join(CHAT_HOOK_INSTALL_NAME);
    std::fs::write(&hook_path, CHAT_HOOK_ASSET)?;
    make_executable(&hook_path)?;
    update_settings(&dir.join("settings.json"), &hook_path, true)?;
    Ok(hook_path)
}

/// Entfernt die Einträge aus `settings.json` und löscht den Hook. Liefert, ob
/// eine Hook-Datei gelöscht wurde.
pub(crate) fn uninstall_for_claude() -> io::Result<bool> {
    let dir = claude_dir()?;
    let hook_path = dir.join("hooks").join(CHAT_HOOK_INSTALL_NAME);
    update_settings(&dir.join("settings.json"), &hook_path, false)?;
    remove_file_if_exists(&hook_path)
}

fn update_settings(settings_path: &Path, hook_path: &Path, installing: bool) -> io::Result<()> {
    let existing = match std::fs::read_to_string(settings_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound && installing => "{}".to_string(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let updated = edit_settings(&existing, hook_path, installing)?;
    if updated != existing {
        write_config(settings_path, updated)?;
    }
    Ok(())
}

fn edit_settings(content: &str, hook_path: &Path, installing: bool) -> io::Result<String> {
    let root = CstRootNode::parse(content, &ParseOptions::default())
        .map_err(|err| io::Error::other(format!("invalid claude settings: {err}")))?;
    let Some(root_object) = root.object_value() else {
        return Err(io::Error::other("claude settings must be a JSON object"));
    };
    let hooks = match (root_object.get("hooks"), installing) {
        (Some(_), _) | (None, true) => root_object.object_value_or_set("hooks"),
        (None, false) => return Ok(content.to_string()),
    };

    let mut changed = false;
    for (event, action) in CHAT_HOOK_EVENTS {
        let command = hook_command(hook_path, Some(action));
        let entries = match (hooks.get(event), installing) {
            (Some(_), _) | (None, true) => hooks.array_value_or_set(event),
            (None, false) => continue,
        };

        let mut installed = false;
        for entry in entries.elements() {
            let commands = entry_commands(&entry);
            if commands.is_empty() {
                continue;
            }
            let ours = commands.iter().all(|c| *c == command);
            let legacy = commands.iter().all(|c| is_chat_hook_command(c, action));
            if installing && ours && !installed {
                installed = true;
            } else if legacy {
                entry.remove();
                changed = true;
            }
        }

        if installing && !installed {
            entries.append(entry_input(&command));
            changed = true;
        }
        if entries.elements().is_empty() {
            if let Some(prop) = hooks.get(event) {
                prop.remove();
                changed = true;
            }
        }
    }

    Ok(if changed {
        root.to_string()
    } else {
        content.to_string()
    })
}

/// Befehle aller Command-Hooks einer Hook-Gruppe. Eine Gruppe mit anderen
/// Hook-Typen liefert eine leere Liste und bleibt unangetastet.
fn entry_commands(entry: &CstNode) -> Vec<String> {
    let Some(Value::Object(entry)) = entry.to_serde_value() else {
        return Vec::new();
    };
    let Some(Value::Array(hooks)) = entry.get("hooks") else {
        return Vec::new();
    };
    let commands: Vec<String> = hooks
        .iter()
        .filter_map(|hook| {
            (hook.get("type").and_then(Value::as_str) == Some("command"))
                .then(|| hook.get("command").and_then(Value::as_str))
                .flatten()
                .map(str::to_string)
        })
        .collect();
    if commands.len() == hooks.len() {
        commands
    } else {
        Vec::new()
    }
}

fn is_chat_hook_command(command: &str, action: &str) -> bool {
    let command = command.trim_end();
    let Some(path) = command.strip_suffix(action).map(str::trim_end) else {
        return false;
    };
    let path = path.trim_matches(|c| c == '\'' || c == '"');
    path.ends_with(CHAT_HOOK_INSTALL_NAME) || path.ends_with(LEGACY_HOOK_NAME)
}

fn entry_input(command: &str) -> CstInputValue {
    json!({
        hooks: [{
            "type": "command",
            command: command,
            timeout: CHAT_HOOK_TIMEOUT,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> PathBuf {
        PathBuf::from("/home/u/.claude/hooks").join(CHAT_HOOK_INSTALL_NAME)
    }

    fn parse(content: &str) -> Value {
        serde_json::from_str(content).unwrap()
    }

    #[test]
    fn install_adds_stop_and_notification_entries() {
        let updated = edit_settings("{}", &hook(), true).unwrap();
        let settings = parse(&updated);
        for (event, action) in CHAT_HOOK_EVENTS {
            assert_eq!(
                settings["hooks"][event][0]["hooks"][0]["command"],
                hook_command(&hook(), Some(action))
            );
        }
    }

    #[test]
    fn install_is_a_byte_exact_noop_when_already_installed() {
        let installed = edit_settings("{\n  \"model\": \"x\"\n}\n", &hook(), true).unwrap();
        assert_eq!(edit_settings(&installed, &hook(), true).unwrap(), installed);
    }

    #[test]
    fn install_replaces_legacy_entries_and_keeps_user_hooks() {
        let content = r#"{
  "hooks": {
    "Stop": [
      { "hooks": [{ "type": "command", "command": "~/projects/collie/scripts/collie/claude-chat-hook.sh stop", "timeout": 5 }] },
      { "hooks": [{ "type": "command", "command": "echo keep" }] }
    ]
  }
}"#;
        let settings = parse(&edit_settings(content, &hook(), true).unwrap());
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "echo keep");
        assert_eq!(
            stop[1]["hooks"][0]["command"],
            hook_command(&hook(), Some("stop"))
        );
    }

    #[test]
    fn uninstall_removes_only_chat_hook_entries() {
        let installed = edit_settings(
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#,
            &hook(),
            true,
        )
        .unwrap();
        let settings = parse(&edit_settings(&installed, &hook(), false).unwrap());
        assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(settings["hooks"].get("Notification").is_none());
    }

    #[test]
    fn uninstall_without_hooks_is_a_noop() {
        let content = "{ \"model\": \"x\" }";
        assert_eq!(edit_settings(content, &hook(), false).unwrap(), content);
    }
}
