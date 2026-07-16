#!/usr/bin/env bash
# sync-creds.sh — refresh the isolated containerized-agent creds COPIES from the host's CURRENT creds,
# as a pre-flight BEFORE a containerized session.
#
# WHY: OAuth subscription (claude) and SSO tokens ROTATE on refresh — the refresh token is often
# single-use. So two independent consumers sharing one token lineage invalidate each other: when you
# use the agent on the host, the host rotates the lineage and the container's COPY (with the now-dead
# refresh token) goes stale (-> `session/prompt failed`). Syncing the host's CURRENT token into the
# copy right before a run lets each short container turn "borrow" a still-valid access token WITHOUT
# refreshing -> no rotation -> the host stays valid too. This script copies bytes and does not prove or
# renew freshness; `a2a-bridge doctor` must green after sync. The copy (vs mounting the host file directly)
# still protects the host from a container write clobbering it.
#
# Run before serve / run-workflow / implement, e.g.:
#     deploy/containers/sync-creds.sh && a2a-bridge serve --config examples/a2a-bridge.containerized.toml
#
# Usage: sync-creds.sh [all|claude|codex|kiro]   (default: all)
set -euo pipefail

DEST="$HOME/.config/a2a-creds"
want="${1:-all}"
case "$want" in all|claude|codex|kiro) ;; *) echo "usage: $(basename "$0") [all|claude|codex|kiro]" >&2; exit 1 ;; esac

sync_file() { # <agent> <host-src> <copy-filename>
  local agent="$1" src="$2" dst="$DEST/$1/$3"
  if [ -f "$src" ]; then
    mkdir -p "$(dirname "$dst")"
    cp "$src" "$dst"
    chmod u+rw "$dst"
    echo "synced $agent  <- $src"
  else
    echo "skip   $agent  (host creds not found at $src)"
  fi
}

# claude (OAuth subscription) + codex (ChatGPT auth) are single host files → sync the copy.
if [ "$want" = all ] || [ "$want" = claude ]; then
  sync_file claude "$HOME/.claude/.credentials.json" ".credentials.json"
fi
if [ "$want" = all ] || [ "$want" = codex ]; then
  sync_file codex "$HOME/.codex/auth.json" "auth.json"
fi

# kiro is DIFFERENT: its Linux container auth lives in the `a2a-kiro-data` named volume (minted
# in-container via the device flow; the host kiro is macOS, a separate lineage), and the volume token
# refreshes itself. Nothing to sync from the host. If it has FULLY expired, re-run the device-flow login.
if [ "$want" = all ] || [ "$want" = kiro ]; then
  echo "note   kiro  = the a2a-kiro-data volume (in-container device-flow; not a host file)."
  echo "             if expired, re-run: ${CONTAINER_RUNTIME:-docker} run -it --rm -v a2a-kiro-data:/root/.local/share a2a-agent-reader:latest kiro-cli login --use-device-flow"
fi
