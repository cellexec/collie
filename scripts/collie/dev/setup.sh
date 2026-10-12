#!/usr/bin/env bash
# collie: Maintainer-Setup für den Hauptcheckout, aufgerufen von `just dev-setup`.
#
# - verlinkt collie-dev nach ~/.local/bin
# - legt <collie-worktrees>/.cargo/config.toml an (gemeinsames Target, mold)
# - trägt den Guard ein, der Edits im Hauptcheckout blockt (.claude/, lokal)
#
# Mehrfach ausführbar. Vorhandene, abweichende Dateien werden nicht überschrieben.
set -euo pipefail

REPO=$(cd "$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)
TREES=${HERDR_WORKTREES:-$(dirname "$REPO")/collie-worktrees}
DEV=$(cd "$(dirname "$0")" && pwd -P)
BIN=${COLLIE_BIN_DIR:-$HOME/.local/bin}

say() { printf '%-8s %s\n' "$1" "$2"; }

# Schreibt $2 nach $1, außer dort liegt schon etwas anderes.
write_once() {
    local target=$1 content=$2
    if [ -e "$target" ]; then
        if [ "$(cat "$target")" = "$content" ]; then
            say ok "$target"
        else
            say skip "$target existiert mit anderem Inhalt"
        fi
        return
    fi
    mkdir -p "$(dirname "$target")"
    printf '%s\n' "$content" > "$target"
    say write "$target"
}

link() {
    local src=$1 target=$2
    if [ -L "$target" ] && [ "$(readlink -f "$target")" = "$(readlink -f "$src")" ]; then
        say ok "$target"
    elif [ -e "$target" ] && [ ! -L "$target" ]; then
        say skip "$target ist eine Datei, kein Symlink"
    else
        mkdir -p "$(dirname "$target")"
        ln -sfn "$src" "$target"
        say link "$target -> $src"
    fi
}

for tool in git cargo fzf jq python3; do
    command -v "$tool" >/dev/null || say missing "$tool"
done

link "$DEV/collie-dev" "$BIN/collie-dev"

cargo_config=$(sed "s|@TREES@|$TREES|" "$DEV/worktrees-cargo-config.toml")
if command -v mold >/dev/null && command -v clang >/dev/null; then
    cargo_config+=$'\n\n# mold statt GNU ld: schnelleres Linken.\n[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]'
fi
write_once "$TREES/.cargo/config.toml" "$cargo_config"

link "$DEV/main-checkout-guard.sh" "$REPO/.claude/hooks/main-checkout-guard.sh"
write_once "$REPO/.claude/settings.local.json" '{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit|NotebookEdit",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/main-checkout-guard.sh"
          }
        ]
      }
    ]
  }
}'

exclude=$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)/info/exclude
for pattern in /.claude/ /CLAUDE.local.md; do
    grep -qxF "$pattern" "$exclude" 2>/dev/null || { echo "$pattern" >> "$exclude"; say exclude "$pattern"; }
done

case ":$PATH:" in
    *":$BIN:"*) ;;
    *) say path "$BIN fehlt im PATH" ;;
esac
