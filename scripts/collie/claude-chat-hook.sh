#!/bin/sh
# collie: meldet letzte Nachricht und Blocked-Grund eines Claude-Chats an den
# Server (`pane.report_chat`). `collie integration install claude` installiert
# es als ~/.claude/hooks/collie-chat-hook.sh, siehe docs/specs/chat-sidebar-overlay.md.
#
#   claude-chat-hook.sh stop          Stop-Hook
#   claude-chat-hook.sh notification  Notification-Hook

set -eu

action="${1:-}"
case "$action" in
  stop | notification) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HERDR_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" exec python3 -I -c '
import json
import os
import re
import socket
import sys
import time

TAIL_BYTES = 256 * 1024


def last_line(text):
    for line in reversed(text.splitlines()):
        line = line.strip()
        line = re.sub(r"^(#+|>+|[-*+]|\d+[.)])\s+", "", line)
        line = line.replace("**", "").replace("`", "").strip()
        if line:
            return line
    return None


def transcript_text(path):
    try:
        with open(path, "rb") as handle:
            handle.seek(0, os.SEEK_END)
            size = handle.tell()
            handle.seek(max(0, size - TAIL_BYTES))
            lines = handle.read().decode("utf-8", "replace").splitlines()
    except OSError:
        return None
    for raw in reversed(lines):
        try:
            entry = json.loads(raw)
        except ValueError:
            continue
        if entry.get("type") != "assistant" or entry.get("isSidechain"):
            continue
        content = (entry.get("message") or {}).get("content")
        if isinstance(content, str):
            return content
        texts = [
            block.get("text", "")
            for block in content or []
            if isinstance(block, dict) and block.get("type") == "text"
        ]
        if texts:
            return texts[-1]
    return None


try:
    hook = json.load(sys.stdin)
except ValueError:
    hook = {}
if not isinstance(hook, dict):
    hook = {}

params = {"pane_id": os.environ["HERDR_PANE_ID"]}
if os.environ.get("HERDR_ACTION") == "stop":
    text = hook.get("last_assistant_message")
    if not isinstance(text, str) or not text.strip():
        path = hook.get("transcript_path")
        text = transcript_text(path) if isinstance(path, str) else None
    line = last_line(text or "")
    if not line:
        raise SystemExit(0)
    params["last_message"] = line
else:
    message = hook.get("message")
    kind = hook.get("notification_type")
    if not isinstance(message, str):
        raise SystemExit(0)
    if kind != "permission_prompt" and "permission" not in message.lower():
        raise SystemExit(0)
    tool = re.search(r"permission to use (.+?)\.?$", message)
    params["blocked_reason"] = f"Freigabe: {tool.group(1)}" if tool else message

request = {
    "id": f"collie:chat:{int(time.time() * 1000)}",
    "method": "pane.report_chat",
    "params": params,
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
'
