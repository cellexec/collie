#!/bin/sh
# collie: baut collie aus den Quellen und richtet es ein.
#
#   curl -fsSL https://raw.githubusercontent.com/cellexec/collie/master/scripts/collie/install.sh | sh
#   just install                  (aus einem Checkout)
#
# Ohne Checkout klont das Skript nach $COLLIE_SRC (Default ~/.local/share/collie/src)
# und aktualisiert ihn bei jedem weiteren Aufruf. Danach:
# - cargo build --release
# - Links collie und herdr in $COLLIE_BIN_DIR (Default ~/.local/bin)
# - collie integration install claude, falls ~/.claude existiert
#
# Ein herdr, das nicht auf collie zeigt, bleibt unangetastet.
set -eu

REPO_URL=${COLLIE_REPO_URL:-https://github.com/cellexec/collie.git}
BIN=${COLLIE_BIN_DIR:-$HOME/.local/bin}
ZIG_VERSION=0.16.0

say() { printf '%-8s %s\n' "$1" "$2"; }
die() { printf 'collie install: %s\n' "$*" >&2; exit 1; }

# Läuft das Skript aus einem Checkout, baut es diesen. Unter `curl | sh` ist $0 die Shell.
src=""
case "$0" in
    *scripts/collie/install.sh)
        [ -f "$0" ] && src=$(cd "$(dirname "$0")/../.." && pwd -P) ;;
esac

for tool in git cargo zig; do
    command -v "$tool" >/dev/null || die "$tool fehlt"
done
zig_version=$(zig version)
[ "$zig_version" = "$ZIG_VERSION" ] || die "zig $ZIG_VERSION nötig, gefunden $zig_version"

if [ -z "$src" ]; then
    src=${COLLIE_SRC:-$HOME/.local/share/collie/src}
    if [ -d "$src/.git" ]; then
        say update "$src"
        git -C "$src" pull --ff-only --quiet
    else
        say clone "$REPO_URL -> $src"
        mkdir -p "$(dirname "$src")"
        git clone --quiet "$REPO_URL" "$src"
    fi
fi

say build "$src"
# Aus $src heraus bauen, damit Cargo dessen Config findet und keine aus dem
# Aufrufverzeichnis. Das Target-Verzeichnis kann per Config oder Env woanders liegen.
cd "$src"
cargo build --release --locked --bin herdr
target_dir=$(cargo metadata --format-version 1 --no-deps --offline \
    | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
binary=${target_dir:-$src/target}/release/herdr
[ -x "$binary" ] || die "Build fertig, aber $binary fehlt"

mkdir -p "$BIN"
ln -sfn "$binary" "$BIN/collie"
say link "$BIN/collie -> $binary"

# Agent-Integrationen rufen herdr beim Namen auf.
existing=$(command -v herdr 2>/dev/null || true)
if [ -z "$existing" ] || [ "$(readlink -f "$existing")" = "$(readlink -f "$binary")" ] \
    || { [ "$existing" = "$BIN/herdr" ] && [ -L "$BIN/herdr" ]; }; then
    ln -sfn "$binary" "$BIN/herdr"
    say link "$BIN/herdr -> $binary"
else
    say skip "$existing zeigt auf $(readlink -f "$existing"), nicht auf diesen Build; Agent-Hooks rufen jenes herdr auf"
fi

if [ -d "${CLAUDE_CONFIG_DIR:-$HOME/.claude}" ]; then
    "$BIN/collie" integration install claude
else
    say skip "kein ~/.claude; später: collie integration install claude"
fi

case ":$PATH:" in
    *":$BIN:"*) ;;
    *) say path "$BIN fehlt im PATH" ;;
esac
say done "collie starten mit: collie"
