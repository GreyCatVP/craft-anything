#!/usr/bin/env bash
# Craft Anything "anywhere" bootstrap (macOS / Linux).
# Downloads the official release CLI (original, unmodified), connects it to detected
# agent hosts, enables Deep mode + local Ghidra, and copies the skill to all host dirs.
# Does not edit any upstream source.
set -euo pipefail
VERSION="${1:-0.5.0}"

find_ghidra() {
  for r in "$HOME/Tools" "$HOME/Downloads" "$HOME/Desktop" "$HOME" /opt /usr/share /Applications "$HOME/Applications"; do
    [ -d "$r" ] || continue
    for d in "$r"/ghidra_*; do
      [ -f "$d/support/analyzeHeadless" ] && { echo "$d"; return; }
    done
  done
}
find_java() {
  if [ -n "${JAVA_HOME:-}" ]; then echo "$JAVA_HOME"; return; fi
  for d in "$HOME"/Tools/jdk*; do [ -d "$d" ] && { echo "$d"; return; }; done
}

# 1. Locate or download the CLI.
CLI="$(command -v craft-anything-cli || true)"
if [ -z "$CLI" ]; then
  for p in "$HOME/.local/bin/craft-anything-cli" "$HOME/Tools/craft-anything/craft-anything-cli"; do
    [ -x "$p" ] && { CLI="$p"; break; }
  done
fi
if [ -z "$CLI" ]; then
  os="$(uname -s)"; arch="$(uname -m)"
  mkdir -p "$HOME/.local/bin" "$HOME/Tools/craft-anything"
  tmp="$(mktemp -d)"
  if [ "$os" = "Darwin" ]; then
    url="https://github.com/storytold/craft-anything/releases/download/v$VERSION/craft-anything-cli-$VERSION-macos-universal.zip"
    curl -fL "$url" -o "$tmp/ca.zip"; unzip -q "$tmp/ca.zip" -d "$tmp/x"
    find "$tmp/x" -name craft-anything-cli -type f -exec cp {} "$HOME/.local/bin/craft-anything-cli" \;
    chmod +x "$HOME/.local/bin/craft-anything-cli"; CLI="$HOME/.local/bin/craft-anything-cli"
  else
    case "$arch" in x86_64) a=x86_64;; aarch64|arm64) a=aarch64;; *) a="$arch";; esac
    url="https://github.com/storytold/craft-anything/releases/download/v$VERSION/craft-anything-$VERSION-linux-$a.tar.gz"
    curl -fL "$url" -o "$tmp/ca.tgz"; tar -xzf "$tmp/ca.tgz" -C "$tmp/x" 2>/dev/null || { mkdir -p "$tmp/x"; tar -xzf "$tmp/ca.tgz" -C "$tmp/x"; }
    find "$tmp/x" -name craft-anything-cli -type f -exec cp {} "$HOME/.local/bin/craft-anything-cli" \;
    chmod +x "$HOME/.local/bin/craft-anything-cli"; CLI="$HOME/.local/bin/craft-anything-cli"
  fi
fi
echo "CLI: $CLI"

# 2. Connect to detected hosts + install bundled skill.
"$CLI" setup --all-detected --yes

# 3. Deep mode + local Ghidra/Java.
"$CLI" settings_set --key deep_mode --value true >/dev/null
G="$(find_ghidra || true)"
if [ -n "$G" ]; then "$CLI" settings_set --key engines.ghidra.install_dir --value "\"$G\"" >/dev/null; echo "Ghidra: $G"; fi
J="$(find_java || true)"
if [ -n "$J" ]; then "$CLI" settings_set --key engines.ghidra.java_home --value "\"$J\"" >/dev/null; fi

# 4. Copy bundled skill into every known host skill dir.
SRC=""
for p in "$HOME/.agents/skills/craft-anything" "$HOME/.qoder/skills/craft-anything"; do
  [ -d "$p" ] && { SRC="$p"; break; }
done
if [ -n "$SRC" ]; then
  for h in .qoder .claude .codex .cursor .agents; do
    mkdir -p "$HOME/$h/skills"; cp -R "$SRC" "$HOME/$h/skills/craft-anything"
  done
  echo "Skill copied to host dirs."
fi

# 5. Summary.
"$CLI" doctor
echo "Done. Restart/reconnect your AI host so its MCP tool list refreshes."