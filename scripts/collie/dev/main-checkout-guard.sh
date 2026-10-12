#!/usr/bin/env bash
# Blockt Datei-Edits im collie-Hauptcheckout; Implementierung gehört in einen
# Worktree (collie-dev new). Erlaubt während eines Merges. Eingetragen von
# `just dev-setup` in .claude/settings.local.json des Hauptcheckouts.
set -euo pipefail

MAIN=$(realpath -m "${CLAUDE_PROJECT_DIR:-$HOME/projects/collie}")
path=$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty')
[ -n "$path" ] || exit 0

case "$(realpath -m "$path")/" in
    "$MAIN"/.claude/* | "$MAIN"/CLAUDE.local.md/) exit 0 ;;
    "$MAIN"/*) ;;
    *) exit 0 ;;
esac

[ -e "$MAIN/.git/MERGE_HEAD" ] && exit 0

echo "Der Hauptcheckout $MAIN bleibt sauber auf master. Lege einen Worktree an (collie-dev new <type>/<name>) und bearbeite die Datei dort." >&2
exit 2
