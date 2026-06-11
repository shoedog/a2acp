#!/usr/bin/env bash
# Podman egress bring-up — the rootless/macOS-podman counterpart of compose.egress.yaml.
# Reproduces the SAME names so the bridge config contract is unchanged:
#   networks: a2a-egress-internal (--internal), a2a-verify-egress (--internal), a2a-egress-external (routed)
#   proxies:  a2a-egress-proxy (internal + external, agent filter), a2a-verify-proxy (verify + external, registries filter)
# Both proxies BIND-MOUNT their tinyproxy filter (always fresh — edit the filter, re-run `up`, no rebuild).
#
# Usage:  deploy/containers/podman-egress.sh up | status | down
#   up:     idempotent. Re-run after every `podman machine start` (proxies do NOT survive a daemonless
#           `podman machine stop/start`). Skips intact running proxies unless FORCE=1.
#   status: list the 3 networks + 2 proxies.
#   down:   tolerate not-found; REPORT any other failure (e.g. a network still in use).
#
# Override the runtime with CONTAINER_RUNTIME=podman (default) — also works with nerdctl/docker.
#
# OLD-PODMAN / IP PINNING (aardvark-dns < netavark 1.6 / podman < 4.5 did NOT serve DNS on --internal
# networks, so name resolution of `a2a-egress-proxy` from the internal net fails). Pin via env so EVERY
# `up` (including the post-machine-restart re-up) reproduces the SAME topology — then set the matching
# proxy URLs in the bridge config:
#   EGRESS_INTERNAL_SUBNET=10.89.0.0/24 EGRESS_PROXY_IP=10.89.0.2 \
#   EGRESS_VERIFY_SUBNET=10.90.0.0/24   EGRESS_VERIFY_PROXY_IP=10.90.0.2  deploy/containers/podman-egress.sh up
#   # then in the podman config:  proxy = "http://10.89.0.2:8888"  (verify: "http://10.90.0.2:8888")
# On podman >= 4.5 leave these unset and use the default name-based URLs.
set -euo pipefail

cd "$(dirname "$0")" # self-locate so the relative filter binds + Containerfile resolve regardless of CWD

RT="${CONTAINER_RUNTIME:-podman}"
INTERNAL_SUBNET="${EGRESS_INTERNAL_SUBNET:-}"
PROXY_IP="${EGRESS_PROXY_IP:-}"
VERIFY_SUBNET="${EGRESS_VERIFY_SUBNET:-}"
VERIFY_PROXY_IP="${EGRESS_VERIFY_PROXY_IP:-}"

# Create an --internal network (idempotent). If it already exists, ASSERT it is actually internal — a
# same-named NON-internal network would silently void containment (agents would have direct egress).
ensure_internal_network() { # $1 = name, $2 = optional subnet
  local name="$1" subnet="${2:-}"
  if "$RT" network exists "$name" 2>/dev/null; then
    local internal
    internal="$("$RT" network inspect "$name" --format '{{.Internal}}' 2>/dev/null || echo "unknown")"
    if [ "$internal" != "true" ]; then
      echo "FATAL: network '$name' exists but is NOT internal (Internal=$internal) — containment would be" >&2
      echo "       bypassed (agents get direct egress). Remove it and re-run: $RT network rm $name" >&2
      exit 1
    fi
  else
    if [ -n "$subnet" ]; then
      "$RT" network create --internal --subnet "$subnet" "$name"
    else
      "$RT" network create --internal "$name"
    fi
  fi
}

ensure_routed_network() { # $1 = name
  "$RT" network exists "$1" 2>/dev/null || "$RT" network create "$1"
}

# A proxy is "intact" iff it is running. This script ALWAYS attaches both nets + binds the filter, so a
# running proxy is correctly wired; a non-disruptive re-up can skip it (FORCE=1 overrides).
proxy_running() { "$RT" ps --filter "name=^$1\$" --filter status=running --format '{{.Names}}' 2>/dev/null | grep -qx "$1"; }

# create+connect+start one proxy: $1=name $2=internal-net $3=filter-file $4=optional-ip
start_proxy() {
  local name="$1" net="$2" filter="$3" ip="${4:-}"
  "$RT" rm -f "$name" >/dev/null 2>&1 || true
  if [ -n "$ip" ]; then
    "$RT" create --name "$name" --network "$net" --ip "$ip" \
      -v "$PWD/$filter:/etc/tinyproxy/filter:ro" a2a-egress-proxy:latest
  else
    "$RT" create --name "$name" --network "$net" \
      -v "$PWD/$filter:/etc/tinyproxy/filter:ro" a2a-egress-proxy:latest
  fi
  "$RT" network connect a2a-egress-external "$name"
  "$RT" start "$name"
}

up() {
  ensure_internal_network a2a-egress-internal "$INTERNAL_SUBNET"
  ensure_internal_network a2a-verify-egress "$VERIFY_SUBNET"
  ensure_routed_network a2a-egress-external

  "$RT" image exists a2a-egress-proxy:latest 2>/dev/null \
    || "$RT" build -t a2a-egress-proxy:latest -f proxy.Containerfile .

  if [ "${FORCE:-0}" != "1" ] && proxy_running a2a-egress-proxy && proxy_running a2a-verify-proxy; then
    echo "egress already up ($RT): both proxies running (FORCE=1 to recreate)"
    return 0
  fi

  start_proxy a2a-egress-proxy a2a-egress-internal tinyproxy.filter        "$PROXY_IP"
  start_proxy a2a-verify-proxy a2a-verify-egress   tinyproxy.verify.filter "$VERIFY_PROXY_IP"

  echo "egress up ($RT): 3 networks + a2a-egress-proxy + a2a-verify-proxy"
  echo "  re-run '$0 up' after every 'podman machine start' (proxies do not survive a machine restart)."
}

status() {
  echo "== networks =="
  "$RT" network ls --filter name=a2a-egress-internal --filter name=a2a-egress-external --filter name=a2a-verify-egress
  echo "== proxies =="
  "$RT" ps -a --filter name=a2a-egress-proxy --filter name=a2a-verify-proxy
}

# Remove a resource, tolerating only "no such" — REPORT anything else (e.g. a network still in use), so a
# half-down state is never silently reported as success.
rm_resource() { # $1 = kind (container|network), $2 = name
  local kind="$1" name="$2" err
  if err="$("$RT" "$kind" rm -f "$name" 2>&1)"; then return 0; fi
  case "$err" in
    *"no such"*|*"not find"*|*"no container"*|*"unable to find network"*) return 0 ;;
    *) echo "WARN: $kind rm $name: $err" >&2; return 1 ;;
  esac
}

down() {
  local rc=0
  rm_resource container a2a-egress-proxy || rc=1
  rm_resource container a2a-verify-proxy || rc=1
  rm_resource network a2a-egress-internal || rc=1
  rm_resource network a2a-verify-egress || rc=1
  rm_resource network a2a-egress-external || rc=1
  [ "$rc" -eq 0 ] && echo "egress down ($RT)" || echo "egress down ($RT) — with warnings (see above)" >&2
  return "$rc"
}

case "${1:-}" in
  up)     up ;;
  status) status ;;
  down)   down ;;
  *) echo "usage: $0 up | status | down   (env: CONTAINER_RUNTIME, FORCE=1, EGRESS_*_SUBNET/IP)" >&2; exit 2 ;;
esac
