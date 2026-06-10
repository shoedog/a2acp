#!/usr/bin/env bash
# Podman egress bring-up — the rootless/macOS-podman counterpart of compose.egress.yaml.
# Reproduces the SAME names so the bridge config contract is unchanged:
#   networks: a2a-egress-internal (--internal), a2a-verify-egress (--internal), a2a-egress-external (routed)
#   proxies:  a2a-egress-proxy (on internal + external), a2a-verify-proxy (on verify + external, registries filter)
#
# Usage:  deploy/containers/podman-egress.sh up | status | down
#   up:     idempotent (re-run after every `podman machine start` — --restart does NOT survive a
#           daemonless `podman machine stop/start`). rm -f each proxy first, so re-up is the recovery path.
#   status: list the 3 networks + 2 proxies.
#   down:   tolerate-absent (proxies before networks).
#
# Override the runtime with CONTAINER_RUNTIME=podman (default) — also works with nerdctl/docker.
#
# Post-condition contract (G2 tests this; it doubles as the compose<->script drift detector):
#   three networks exist (two internal with no external route, one routed); both proxies run attached to
#   their internal net + the external net; each proxy is reachable from its internal net at exactly the URL
#   the bridge config states (http://a2a-egress-proxy:8888, http://a2a-verify-proxy:8888); the verify proxy
#   serves the registries-only filter.
#
# INTERNAL-NET DNS FALLBACK (aardvark-dns < netavark 1.6 / podman < 4.5 did NOT serve DNS on --internal
# networks). If name resolution of `a2a-egress-proxy` from the internal net fails, pin IPs instead:
#   - create internal nets with a subnet:  $RT network create --internal --subnet 10.89.0.0/24 a2a-egress-internal
#                                           $RT network create --internal --subnet 10.90.0.0/24 a2a-verify-egress
#   - create each proxy with --ip:          $RT create ... --ip 10.89.0.2 ...   (verify: --ip 10.90.0.2)
#   - set the bridge config proxy URL:      proxy = "http://10.89.0.2:8888"  (verify: "http://10.90.0.2:8888")
# Name-based first (debuggable); IP-pinned only if DNS is unavailable.
set -euo pipefail

# Self-locate so the relative filter bind + Containerfile resolve regardless of CWD.
cd "$(dirname "$0")"

RT="${CONTAINER_RUNTIME:-podman}"

ensure_network() { # $1 = name, $2... = extra flags (e.g. --internal)
  local name="$1"; shift
  "$RT" network exists "$name" 2>/dev/null || "$RT" network create "$@" "$name"
}

up() {
  # Networks (idempotent).
  ensure_network a2a-egress-internal --internal
  ensure_network a2a-egress-external
  ensure_network a2a-verify-egress --internal

  # Proxy image (build into podman's store if absent — separate from docker's).
  "$RT" image exists a2a-egress-proxy:latest 2>/dev/null \
    || "$RT" build -t a2a-egress-proxy:latest -f proxy.Containerfile .

  # agent egress proxy: internal + external, default (provider-only) filter.
  "$RT" rm -f a2a-egress-proxy >/dev/null 2>&1 || true
  "$RT" create --name a2a-egress-proxy --network a2a-egress-internal a2a-egress-proxy:latest
  "$RT" network connect a2a-egress-external a2a-egress-proxy
  "$RT" start a2a-egress-proxy

  # verify egress proxy: verify-internal + external, registries-only filter (absolute bind for podman).
  "$RT" rm -f a2a-verify-proxy >/dev/null 2>&1 || true
  "$RT" create --name a2a-verify-proxy --network a2a-verify-egress \
    -v "$PWD/tinyproxy.verify.filter:/etc/tinyproxy/filter:ro" a2a-egress-proxy:latest
  "$RT" network connect a2a-egress-external a2a-verify-proxy
  "$RT" start a2a-verify-proxy

  echo "egress up ($RT): 3 networks + a2a-egress-proxy + a2a-verify-proxy"
  echo "  re-run '$0 up' after every 'podman machine start' (proxies do not survive a machine restart)."
}

status() {
  echo "== networks =="
  "$RT" network ls --filter name=a2a-egress-internal --filter name=a2a-egress-external --filter name=a2a-verify-egress
  echo "== proxies =="
  "$RT" ps -a --filter name=a2a-egress-proxy --filter name=a2a-verify-proxy
}

down() {
  "$RT" rm -f a2a-egress-proxy a2a-verify-proxy >/dev/null 2>&1 || true
  "$RT" network rm a2a-egress-internal a2a-egress-external a2a-verify-egress >/dev/null 2>&1 || true
  echo "egress down ($RT)"
}

case "${1:-}" in
  up)     up ;;
  status) status ;;
  down)   down ;;
  *) echo "usage: $0 up | status | down" >&2; exit 2 ;;
esac
