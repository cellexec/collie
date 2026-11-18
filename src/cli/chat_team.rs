//! collie: `herdr chat` für Master Chats und ihre Children, siehe
//! docs/specs/master-chat.md.

use crate::api::schema::{
    ChatCloseChildren, ChatCloseParams, ChatInboxParams, ChatMasterCreateParams, ChatReportParams,
    ChatSpawnParams, ChatTellUserParams, ChatUserInboxActParams, ChatUserInboxAction,
    ChatUserInboxParams, ChatWorkspaceParams, EmptyParams, Method, PaneCurrentParams, Request,
    WorktreeCreateParams,
};

pub(super) fn print_team_help() {
    eprintln!("  herdr chat master [--cwd DIR] [--no-focus]");
    eprintln!("  herdr chat promote [WORKSPACE]");
    eprintln!("  herdr chat demote [WORKSPACE]");
    eprintln!(
        "  herdr chat spawn --prompt TEXT [--cwd DIR] [--worktree BRANCH [--base REF]] [--title TEXT] [--agent NAME]"
    );
    eprintln!("  herdr chat children [--master WORKSPACE] [--json]");
    eprintln!("  herdr chat stop|resume <CHILD>");
    eprintln!("  herdr chat close <WORKSPACE> [--release-children]");
    eprintln!("  herdr chat report <TEXT>");
    eprintln!("  herdr chat inbox [--all] [--json]");
    eprintln!(
        "  herdr chat tell-user [--kind nachricht|formular|wizard] [--optional] [--title TEXT] [--file PATH] <TEXT|->"
    );
    eprintln!("  herdr chat user-inbox [--master WORKSPACE] [--json]");
}

/// Führt einen Master-Chat-Befehl aus. `None`, wenn `command` keiner ist.
pub(super) fn run_team_command(command: &str, args: &[String]) -> Option<std::io::Result<i32>> {
    let result = match command {
        "master" => master(args),
        "promote" => workspace_command(args, Method::ChatPromote),
        "demote" => workspace_command(args, Method::ChatDemote),
        "spawn" => spawn(args),
        "children" => children(args),
        "stop" => workspace_command(args, Method::ChatStop),
        "resume" => workspace_command(args, Method::ChatResume),
        "close" => close(args),
        "report" => report(args),
        "inbox" => inbox(args),
        "tell-user" => tell_user(args),
        "user-inbox" => user_inbox(args),
        "user-inbox-result" => user_inbox_result(args),
        _ => return None,
    };
    Some(result)
}

/// Optionen der Form `--name WERT` und Flags der Form `--name`.
struct Args {
    positional: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], with_value: &[&str]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let Some(name) = arg.strip_prefix("--") else {
                positional.push(arg.clone());
                continue;
            };
            if with_value.contains(&name) {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                options.push((name.to_owned(), Some(value.clone())));
            } else {
                options.push((name.to_owned(), None));
            }
        }
        Ok(Self {
            positional,
            options,
        })
    }

    fn value(&self, name: &str) -> Option<String> {
        self.options
            .iter()
            .rev()
            .find(|(option, _)| option == name)
            .and_then(|(_, value)| value.clone())
    }

    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(option, _)| option == name)
    }
}

fn usage_error(message: impl std::fmt::Display) -> std::io::Result<i32> {
    eprintln!("{message}");
    print_team_help();
    Ok(2)
}

fn request(id: &str, method: Method) -> std::io::Result<serde_json::Value> {
    super::send_request(&Request {
        id: format!("cli:chat:{id}"),
        method,
    })
}

/// Gibt einen Fehler aus und liefert den Exit-Code, sonst `None`.
fn failed(response: &serde_json::Value) -> Option<i32> {
    let error = response.get("error")?;
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("request failed");
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("error");
    eprintln!("{code}: {message}");
    Some(1)
}

fn caller() -> Option<String> {
    super::target::caller_pane_id()
}

fn print_workspace(response: &serde_json::Value) {
    let workspace = response.pointer("/result/workspace");
    let id = workspace
        .and_then(|workspace| workspace.get("workspace_id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let label = workspace
        .and_then(|workspace| workspace.get("label"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    println!("{id}\t{label}");
}

fn master(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["cwd"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let response = request(
        "master",
        Method::ChatMasterCreate(ChatMasterCreateParams {
            cwd: args.value("cwd"),
            focus: !args.flag("no-focus"),
            adopt_workspace_ids: Vec::new(),
        }),
    )?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    print_workspace(&response);
    Ok(0)
}

/// Befehle mit optionalem Workspace; ohne gilt der Space des Aufrufers.
fn workspace_command(
    args: &[String],
    method: impl FnOnce(ChatWorkspaceParams) -> Method,
) -> std::io::Result<i32> {
    let workspace_id = args.first().filter(|arg| !arg.starts_with("--")).cloned();
    let response = request(
        "workspace",
        method(ChatWorkspaceParams {
            caller_pane_id: workspace_id.is_none().then(caller).flatten(),
            workspace_id,
        }),
    )?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    if response.pointer("/result/workspace").is_some() {
        print_workspace(&response);
    }
    Ok(0)
}

fn spawn(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(
        args,
        &[
            "prompt", "cwd", "worktree", "base", "title", "agent", "master",
        ],
    ) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let Some(prompt) = args
        .value("prompt")
        .or_else(|| (!args.positional.is_empty()).then(|| args.positional.join(" ")))
    else {
        return usage_error("herdr chat spawn needs --prompt");
    };
    let base = ChatSpawnParams {
        master_workspace_id: args.value("master"),
        caller_pane_id: caller(),
        prompt,
        title: args.value("title"),
        agent: args.value("agent"),
        ..Default::default()
    };
    let mut workspace_id = None;
    if let Some(branch) = args.value("worktree") {
        // Erst prüfen, ob noch ein Child starten darf, damit kein verwaister
        // Worktree entsteht.
        let check = request(
            "spawn:check",
            Method::ChatSpawn(ChatSpawnParams {
                check_only: true,
                ..base.clone()
            }),
        )?;
        if let Some(code) = failed(&check) {
            return Ok(code);
        }
        let cwd = args.value("cwd").or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|dir| dir.display().to_string())
        });
        let created = request(
            "spawn:worktree",
            Method::WorktreeCreate(WorktreeCreateParams {
                cwd,
                branch: Some(branch),
                base: args.value("base"),
                label: args.value("title"),
                focus: false,
                ..Default::default()
            }),
        )?;
        if let Some(code) = failed(&created) {
            return Ok(code);
        }
        workspace_id = created
            .pointer("/result/workspace/workspace_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if workspace_id.is_none() {
            eprintln!("worktree.create returned no workspace");
            return Ok(1);
        }
    }
    let response = request(
        "spawn",
        Method::ChatSpawn(ChatSpawnParams {
            workspace_id,
            cwd: args.value("cwd"),
            ..base
        }),
    )?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    print_workspace(&response);
    Ok(0)
}

fn children(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["master"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let master = match args.value("master") {
        Some(master) => master,
        None => {
            let response = request(
                "children:current",
                Method::PaneCurrent(PaneCurrentParams {
                    caller_pane_id: caller(),
                }),
            )?;
            if let Some(code) = failed(&response) {
                return Ok(code);
            }
            let Some(id) = response
                .pointer("/result/pane/workspace_id")
                .and_then(serde_json::Value::as_str)
            else {
                return usage_error("run this inside the master chat or pass --master");
            };
            id.to_owned()
        }
    };
    let response = request("children", Method::WorkspaceList(EmptyParams {}))?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    let workspaces = response
        .pointer("/result/workspaces")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let children = workspaces
        .into_iter()
        .filter(|workspace| {
            workspace
                .pointer("/team/parent_workspace_id")
                .and_then(serde_json::Value::as_str)
                == Some(master.as_str())
        })
        .collect::<Vec<_>>();
    if args.flag("json") {
        println!("{}", serde_json::Value::Array(children));
        return Ok(0);
    }
    let now = crate::workspace::unix_millis_now();
    for child in &children {
        let text = |key: &str| {
            child
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let status = if child.get("stopped_agent").is_some() {
            "stopped".to_owned()
        } else {
            text("agent_status")
        };
        let idle = child
            .get("attention_since")
            .and_then(serde_json::Value::as_u64)
            .filter(|_| status != "working")
            .map(|since| format!("{}m", now.saturating_sub(since) / 60_000))
            .unwrap_or_default();
        let detail = if status == "blocked" {
            text("blocked_reason")
        } else {
            text("last_message")
        };
        println!(
            "{}\t{}\t{}\t{}\t{}",
            text("workspace_id"),
            status,
            idle,
            text("label"),
            detail
        );
    }
    Ok(0)
}

fn close(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &[]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let Some(workspace_id) = args.positional.first().cloned() else {
        return usage_error("herdr chat close needs a workspace");
    };
    let response = request(
        "close",
        Method::ChatClose(ChatCloseParams {
            workspace_id,
            children: if args.flag("release-children") {
                ChatCloseChildren::Release
            } else {
                ChatCloseChildren::Close
            },
            caller_pane_id: caller(),
        }),
    )?;
    Ok(failed(&response).unwrap_or(0))
}

fn report(args: &[String]) -> std::io::Result<i32> {
    let text = args.join(" ");
    if text.trim().is_empty() {
        return usage_error("herdr chat report needs a message");
    }
    let response = request(
        "report",
        Method::ChatReport(ChatReportParams {
            workspace_id: None,
            caller_pane_id: caller(),
            text,
            from_user: false,
        }),
    )?;
    Ok(failed(&response).unwrap_or(0))
}

fn inbox(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["master"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let response = request(
        "inbox",
        Method::ChatInbox(ChatInboxParams {
            workspace_id: args.value("master"),
            caller_pane_id: caller(),
            all: args.flag("all"),
        }),
    )?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    let entries = response
        .pointer("/result/entries")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    if args.flag("json") {
        println!("{entries}");
        return Ok(0);
    }
    for entry in entries.as_array().into_iter().flatten() {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let delivered = entry
            .get("delivered")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        println!(
            "{}{}\t{}\t{}\t{}",
            if delivered { " " } else { "*" },
            text("kind"),
            text("workspace_id"),
            text("title"),
            text("text")
        );
    }
    Ok(0)
}

/// Der Master legt etwas in die Nutzer-Inbox. `-` liest den Text von stdin.
fn tell_user(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["kind", "title", "file", "master"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let mut text = args.positional.join(" ");
    if text == "-" {
        text.clear();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    }
    let kind = args.value("kind").unwrap_or_else(|| "nachricht".to_owned());
    // Der Server läuft in einem anderen Verzeichnis; relative Pfade gelten
    // für den Aufrufer.
    let path = match args.value("file").map(std::fs::canonicalize).transpose() {
        Ok(path) => path.map(|path| path.to_string_lossy().into_owned()),
        Err(err) => return usage_error(format!("--file: {err}")),
    };
    let response = request(
        "tell-user",
        Method::ChatTellUser(ChatTellUserParams {
            workspace_id: args.value("master"),
            caller_pane_id: caller(),
            kind,
            important: !args.flag("optional"),
            title: args.value("title"),
            text,
            path,
        }),
    )?;
    Ok(failed(&response).unwrap_or(0))
}

fn user_inbox(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["master"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let response = request(
        "user-inbox",
        Method::ChatUserInbox(ChatUserInboxParams {
            workspace_id: args.value("master"),
            caller_pane_id: caller(),
            all: false,
        }),
    )?;
    if let Some(code) = failed(&response) {
        return Ok(code);
    }
    let entries = response
        .pointer("/result/user_inbox")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    if args.flag("json") {
        println!("{entries}");
        return Ok(0);
    }
    for entry in entries.as_array().into_iter().flatten() {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let flag = |key: &str| {
            entry
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let state = match text("kind").as_str() {
            "formular" | "wizard" if flag("finished") => "erledigt",
            "formular" | "wizard" => "offen",
            _ if flag("read") => "gelesen",
            _ => "neu",
        };
        println!(
            "{}\t{}\t{}\t{}\t{}",
            entry
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            text("kind"),
            if flag("important") {
                "wichtig"
            } else {
                "optional"
            },
            state,
            text("title"),
        );
    }
    Ok(0)
}

/// Intern: Panes, die collie für Formulare, Wizards und Rückfragen öffnet,
/// melden damit ihr Ergebnis.
fn user_inbox_result(args: &[String]) -> std::io::Result<i32> {
    let args = match Args::parse(args, &["workspace", "exit", "reply"]) {
        Ok(args) => args,
        Err(err) => return usage_error(err),
    };
    let Some(id) = args
        .positional
        .first()
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return usage_error("herdr chat user-inbox-result needs an entry id");
    };
    let (action, exit_code, path) = if args.flag("answered") {
        (ChatUserInboxAction::Answered, None, None)
    } else if let Some(code) = args.value("exit") {
        (
            ChatUserInboxAction::WizardResult,
            Some(code.trim().parse::<i32>().unwrap_or(-1)),
            None,
        )
    } else if let Some(file) = args.value("reply") {
        (ChatUserInboxAction::ReplyWritten, None, Some(file))
    } else {
        return usage_error("use --answered, --exit CODE or --reply FILE");
    };
    let response = request(
        "user-inbox-result",
        Method::ChatUserInboxAct(ChatUserInboxActParams {
            workspace_id: args.value("workspace"),
            caller_pane_id: caller(),
            action,
            ids: vec![id],
            exit_code,
            path,
            all: false,
        }),
    )?;
    Ok(failed(&response).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_split_options_flags_and_positionals() {
        let args = Args::parse(
            &[
                "--prompt".into(),
                "Baue X".into(),
                "--all".into(),
                "w3".into(),
            ],
            &["prompt"],
        )
        .unwrap();
        assert_eq!(args.value("prompt").as_deref(), Some("Baue X"));
        assert!(args.flag("all"));
        assert_eq!(args.positional, ["w3"]);
        assert!(Args::parse(&["--prompt".into()], &["prompt"]).is_err());
    }
}
