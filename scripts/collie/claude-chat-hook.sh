#!/bin/sh
# collie: meldet Chat-Fakten eines Claude-Chats an den Server
# (`pane.report_chat`). `collie integration install claude` installiert es als
# ~/.claude/hooks/collie-chat-hook.sh, siehe docs/specs/chat-sidebar-overlay.md
# und docs/specs/chat-workflow.md.
#
#   claude-chat-hook.sh stop          Stop-Hook: letzte Nachricht, ab und zu ein neuer Titel
#   claude-chat-hook.sh notification  Notification-Hook: Grund einer Freigabe-Anfrage
#   claude-chat-hook.sh status        Statusline: Verzeichnis, Modell und Kontext, gibt nichts aus

set -eu

action="${1:-}"
case "$action" in
  stop | notification | status) ;;
  *) exit 0 ;;
esac

if [ "${HERDR_ENV:-}" != "1" ] || [ -z "${HERDR_SOCKET_PATH:-}" ] || [ -z "${HERDR_PANE_ID:-}" ]; then
  # Die Statusline liest ihr stdin trotzdem, damit Claude Code nicht blockiert.
  [ "$action" = "status" ] && cat >/dev/null
  exit 0
fi
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" exec python3 -I -c '
import json
import os
import re
import socket
import subprocess
import sys
import time

TAIL_BYTES = 256 * 1024
# Für den Titel liest der Hook weiter zurück, bis genug Text da ist. Lange
# Turns bestehen fast nur aus Tool-Ausgaben.
TITLE_TAIL_BYTES = (256 * 1024, 2 * 1024 * 1024, 8 * 1024 * 1024)
TITLE_HEAD_BYTES = 128 * 1024
TITLE_MIN_CHARS = 1500
# Nach dem ersten Turn gibt es einen Titel, danach alle TITLE_EVERY Turns einen neuen.
TITLE_EVERY = 3
TITLE_EXCERPT_CHARS = 6000
TITLE_PROMPT = (
    "Du benennst Chats zwischen einem Entwickler und einem Coding-Agent. "
    "Antworte nur mit einem Titel aus höchstens 6 Wörtern, in der Sprache des Chats, "
    "ohne Anführungszeichen und ohne Satzzeichen am Ende. Der Titel nennt das Thema, "
    "an dem gerade gearbeitet wird. Passt der bisherige Titel noch, gib ihn unverändert zurück."
)


def last_line(text):
    for line in reversed(text.splitlines()):
        line = line.strip()
        line = re.sub(r"^(#+|>+|[-*+]|\d+[.)])\s+", "", line)
        line = line.replace("**", "").replace("`", "").strip()
        if line:
            return line
    return None


def transcript_entries(path, tail_bytes=TAIL_BYTES, head=False):
    """Einträge vom Ende des Transcripts, mit head=True vom Anfang."""
    try:
        with open(path, "rb") as handle:
            if head:
                lines = handle.read(tail_bytes).decode("utf-8", "replace").splitlines()[:-1]
            else:
                handle.seek(0, os.SEEK_END)
                size = handle.tell()
                handle.seek(max(0, size - tail_bytes))
                lines = handle.read().decode("utf-8", "replace").splitlines()
    except OSError:
        return []
    entries = []
    for raw in lines:
        try:
            entry = json.loads(raw)
        except ValueError:
            continue
        if isinstance(entry, dict) and not entry.get("isSidechain"):
            entries.append(entry)
    return entries


def message_text(entry):
    """Sichtbarer Text einer Nutzer- oder Agent-Nachricht, sonst None."""
    if entry.get("type") not in ("user", "assistant") or entry.get("isMeta"):
        return None
    content = (entry.get("message") or {}).get("content")
    if isinstance(content, str):
        text = content
    else:
        text = "\n".join(
            block.get("text", "")
            for block in content or []
            if isinstance(block, dict) and block.get("type") == "text"
        )
    text = text.strip()
    if not text or text.startswith("<"):
        return None
    return text


def last_assistant_text(entries):
    for entry in reversed(entries):
        if entry.get("type") == "assistant":
            text = message_text(entry)
            if text:
                return text
    return None


def send(params):
    request = {
        "id": f"collie:chat:{int(time.time() * 1000)}",
        "method": "pane.report_chat",
        "params": dict(params, pane_id=os.environ["HERDR_PANE_ID"]),
    }
    try:
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(0.5)
        client.connect(os.environ["HERDR_SOCKET_PATH"])
        client.sendall((json.dumps(request) + "\n").encode())
        try:
            client.recv(4096)
        except OSError:
            pass
        client.close()
    except OSError:
        pass


def state_path(name):
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(os.path.expanduser("~"), ".local", "state")
    return os.path.join(base, "collie", name)


def runtime_path(name):
    base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
    return os.path.join(base, f"collie-{os.getuid()}-{name}")


def read_json(path):
    try:
        with open(path) as handle:
            value = json.load(handle)
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def write_json(path, value):
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        tmp = f"{path}.{os.getpid()}"
        with open(tmp, "w") as handle:
            json.dump(value, handle)
        os.replace(tmp, path)
    except OSError:
        pass


def entry_texts(entries):
    texts = []
    for entry in entries:
        text = message_text(entry)
        if text:
            who = "Nutzer" if entry.get("type") == "user" else "Agent"
            texts.append(f"{who}: {text[:600]}")
    return texts


def title_excerpt(path):
    """Erste Nutzer-Nachricht und die jüngsten Nachrichten des Chats."""
    first = next(
        (text for text in entry_texts(transcript_entries(path, TITLE_HEAD_BYTES, head=True))
         if text.startswith("Nutzer: ")),
        None,
    )
    texts = []
    for tail_bytes in TITLE_TAIL_BYTES:
        texts = entry_texts(transcript_entries(path, tail_bytes))
        if sum(len(text) for text in texts) >= TITLE_MIN_CHARS:
            break
    if first and first not in texts:
        texts.insert(0, first)
    excerpt = []
    used = 0
    for text in reversed(texts):
        if used + len(text) > TITLE_EXCERPT_CHARS and excerpt:
            break
        excerpt.append(text)
        used += len(text)
    return "\n\n".join(reversed(excerpt))


def generate_title(path, previous):
    """Fragt Haiku nach einem Titel. Läuft im abgekoppelten Kind-Prozess."""
    excerpt = title_excerpt(path)
    if not excerpt:
        return None
    previous = previous or "(keiner)"
    prompt = f"Bisheriger Titel: {previous}\n\nChat:\n{excerpt}"
    # Ohne HERDR_* melden die Hooks des Kind-Prozesses nichts an collie.
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("HERDR_") and key != "CLAUDECODE"
    }
    try:
        result = subprocess.run(
            [
                "claude", "-p",
                "--model", "haiku",
                "--no-session-persistence",
                "--setting-sources", "",
                "--tools", "",
                "--system-prompt", TITLE_PROMPT,
            ],
            input=prompt,
            capture_output=True,
            text=True,
            timeout=90,
            env=env,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    title = next((line.strip() for line in result.stdout.splitlines() if line.strip()), "")
    title = title.strip("\"„“”«»*` ").rstrip(".!?:;")
    if not title or len(title) > 80:
        return None
    return title


def maybe_retitle(hook):
    session = hook.get("session_id")
    transcript = hook.get("transcript_path")
    if not isinstance(session, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", session):
        return
    if not isinstance(transcript, str):
        return
    path = state_path(f"titles/{session}.json")
    state = read_json(path)
    turns = int(state.get("turns", 0)) + 1
    state["turns"] = turns
    write_json(path, state)
    if turns != 1 and turns % TITLE_EVERY != 0:
        return
    # Doppelt forken, damit der Hook sofort zurückkehrt und Claude Code nicht
    # auf Haiku wartet.
    if os.fork() != 0:
        return
    os.setsid()
    if os.fork() != 0:
        os._exit(0)
    devnull = os.open(os.devnull, os.O_RDWR)
    for fd in (0, 1, 2):
        os.dup2(devnull, fd)
    try:
        title = generate_title(transcript, state.get("title"))
        if title:
            state = read_json(path)
            state["title"] = title
            write_json(path, state)
            send({"title": title})
    finally:
        os._exit(0)


def on_stop(hook):
    path = hook.get("transcript_path")
    entries = transcript_entries(path) if isinstance(path, str) else []
    text = hook.get("last_assistant_message")
    if not isinstance(text, str) or not text.strip():
        text = last_assistant_text(entries)
    line = last_line(text or "")
    if line:
        send({"last_message": line})
    maybe_retitle(hook)


def on_notification(hook):
    message = hook.get("message")
    kind = hook.get("notification_type")
    if not isinstance(message, str):
        return
    if kind != "permission_prompt" and "permission" not in message.lower():
        return
    tool = re.search(r"permission to use (.+?)\.?$", message)
    send({"blocked_reason": f"Freigabe: {tool.group(1)}" if tool else message})


def context_from_transcript(path):
    for entry in reversed(transcript_entries(path)):
        usage = (entry.get("message") or {}).get("usage") if entry.get("type") == "assistant" else None
        if isinstance(usage, dict):
            return sum(
                int(usage.get(key) or 0)
                for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")
            )
    return None


def on_status(hook):
    params = {}
    workspace = hook.get("workspace") or {}
    cwd = workspace.get("current_dir") or hook.get("cwd")
    if isinstance(cwd, str) and cwd:
        params["cwd"] = cwd
    model = hook.get("model") or {}
    name = model.get("display_name") or model.get("id")
    if isinstance(name, str) and name:
        params["model"] = name
    window = hook.get("context_window") or {}
    usage = window.get("current_usage")
    tokens = None
    if isinstance(usage, dict):
        tokens = sum(
            int(usage.get(key) or 0)
            for key in ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")
        )
    elif isinstance(hook.get("transcript_path"), str):
        tokens = context_from_transcript(hook["transcript_path"])
    if tokens is not None:
        params["context_tokens"] = tokens
        size = window.get("context_window_size")
        if not isinstance(size, int) or size <= 0:
            model_id = str(model.get("id") or "")
            size = 1_000_000 if "[1m]" in model_id.lower() or "1M" in str(name) else 200_000
        params["context_window"] = size
    if not params:
        return
    # Die Statusline läuft oft. Gemeldet wird nur, was sich geändert hat.
    pane = os.environ["HERDR_PANE_ID"].replace(":", "_")
    path = runtime_path(f"status-{pane}.json")
    if read_json(path) == params:
        return
    write_json(path, params)
    send(params)


try:
    hook = json.load(sys.stdin)
except ValueError:
    hook = {}
if not isinstance(hook, dict):
    hook = {}

action = os.environ.get("HERDR_ACTION")
if action == "stop":
    on_stop(hook)
elif action == "notification":
    on_notification(hook)
elif action == "status":
    on_status(hook)
'
