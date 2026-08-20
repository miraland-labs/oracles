#!/usr/bin/env bash
# Deploy a Dockerized oracle unit on this host.
#
# Builds a SHA-tagged image of `oracle-<family>-<cluster>:<sha>`, retags it as
# `oracle-<family>-<cluster>:current` (the tag every systemd unit references),
# restarts the named systemd unit, and probes /health. On health failure,
# the previous SHA tag is restored as `:current` and the unit is
# restarted, so a bad build never leaves you with a broken oracle.
#
# Usage:
#   sudo bash oracle-deploy.sh                                       # build + deploy onchain-transfer-devnet
#   sudo bash oracle-deploy.sh --unit oracle-onchain-transfer-mainnet
#   sudo bash oracle-deploy.sh --unit oracle-file-delivery-devnet
#   sudo bash oracle-deploy.sh --skip-build                          # redeploy current SHA (re-tag only)
#   sudo bash oracle-deploy.sh --rollback                            # restore the previous :current tag
#
# Flags:
#   --unit <name>          systemd unit base name (default: oracle-onchain-transfer-devnet).
#                          The script derives the family from the unit prefix
#                          ("oracle-<family>-..."), and reads the unit's
#                          BIND_ADDR (port) from /etc/oracle/<unit-suffix>.env
#                          to pick the /health probe endpoint.
#   --family <name>        Override the family (api-quality | onchain-transfer | file-delivery).
#                          Auto-detected from --unit when not given.
#   --health-port <port>   Override the /health probe port. Auto-detected
#                          from the env file's BIND_ADDR when not given.
#   --health-timeout <s>   Probe deadline; default 30s.
#   --skip-build           Don't rebuild; expect oracle-<family>-<cluster>:<sha> to be local.
#   --rollback             Restore oracle-<family>:previous → :current and restart.

set -euo pipefail

# ----- Defaults --------------------------------------------------------------

UNIT="oracle-onchain-transfer-devnet"
FAMILY=""        # auto-detected from $UNIT below
HEALTH_PORT=""   # auto-detected from env file's BIND_ADDR below
HEALTH_TIMEOUT=30
SKIP_BUILD=0
ROLLBACK=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --unit)            UNIT="$2"; shift 2;;
        --unit=*)          UNIT="${1#*=}"; shift;;
        --family)          FAMILY="$2"; shift 2;;
        --family=*)        FAMILY="${1#*=}"; shift;;
        --health-port)     HEALTH_PORT="$2"; shift 2;;
        --health-port=*)   HEALTH_PORT="${1#*=}"; shift;;
        --health-timeout)  HEALTH_TIMEOUT="$2"; shift 2;;
        --health-timeout=*) HEALTH_TIMEOUT="${1#*=}"; shift;;
        --skip-build)      SKIP_BUILD=1; shift;;
        --rollback)        ROLLBACK=1; shift;;
        -h|--help)
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -28
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 64;;
    esac
done

# Strip the leading "oracle-" so $UNIT_SUFFIX is e.g. "onchain-transfer-devnet".
UNIT_SUFFIX="${UNIT#oracle-}"
ENV_FILE="/etc/oracle/${UNIT_SUFFIX}.env"

# Derive Solana cluster from the unit suffix. Required — each cluster gets its
# own Docker image tag so devnet and mainnet deploys on one host never fight
# over a shared :current tag.
detect_cluster() {
    case "$UNIT_SUFFIX" in
        *-devnet)   echo devnet ;;
        *-mainnet)  echo mainnet ;;
        *-testnet)  echo testnet ;;
        *)          echo "" ;;
    esac
}

CLUSTER="$(detect_cluster)"
if [[ -z "$CLUSTER" ]]; then
    echo "could not derive cluster from --unit=${UNIT}" >&2
    echo "unit suffix must end in -devnet, -mainnet, or -testnet" >&2
    exit 64
fi

# Devnet builds enable sla-escrow-api's compile-time devnet program id.
CARGO_FEATURES=""
if [[ "$CLUSTER" == devnet ]]; then
    CARGO_FEATURES="devnet"
fi

# Auto-detect FAMILY from the unit name. Walk known families and pick the
# first prefix match. Operator can override with --family.
if [[ -z "$FAMILY" ]]; then
    for f in onchain-transfer api-quality file-delivery; do
        if [[ "$UNIT_SUFFIX" == "$f"* ]]; then FAMILY="$f"; break; fi
    done
fi
if [[ -z "$FAMILY" ]]; then
    echo "could not derive --family from --unit=$UNIT; pass --family explicitly" >&2
    exit 64
fi

IMAGE="oracle-${FAMILY}-${CLUSTER}"
SERVICE="${UNIT}.service"

# ----- Workspace root resolution --------------------------------------------
# Prefer git's `rev-parse --show-toplevel` (works whether the script was
# invoked via the symlink or copied elsewhere). Falls back to the script's
# parent path if git is unavailable.
WORKSPACE_ROOT=""
if command -v git >/dev/null 2>&1; then
    if WORKSPACE_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel 2>/dev/null)"; then
        :
    fi
fi
if [[ -z "$WORKSPACE_ROOT" ]]; then
    WORKSPACE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fi
if [[ ! -f "${WORKSPACE_ROOT}/Cargo.toml" ]]; then
    echo "WORKSPACE_ROOT=${WORKSPACE_ROOT} does not contain Cargo.toml" >&2
    exit 65
fi

# ----- Helpers ---------------------------------------------------------------

require_root() {
    if [[ $EUID -ne 0 ]]; then
        echo "must run as root (uses systemctl + docker)" >&2
        exit 77
    fi
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 65; }
}

# Read the env file for BIND_ADDR's port; fall back to family defaults.
# Also surfaces the bound interface to drive the post-deploy reachability
# warning further down.
BIND_INTERFACE=""        # `127.0.0.1`, `0.0.0.0`, or empty when not in env.
detect_health_port() {
    if [[ -n "$HEALTH_PORT" ]]; then return; fi
    if [[ -f "$ENV_FILE" ]]; then
        local bind
        bind="$(grep -E '^BIND_ADDR=' "$ENV_FILE" | head -1 | cut -d= -f2-)"
        if [[ "$bind" =~ ^([0-9.]+):([0-9]+)$ ]]; then
            BIND_INTERFACE="${BASH_REMATCH[1]}"
            HEALTH_PORT="${BASH_REMATCH[2]}"
            return
        fi
    fi
    case "$FAMILY" in
        api-quality)       HEALTH_PORT=4020;;
        onchain-transfer)  HEALTH_PORT=4021;;
        file-delivery)     HEALTH_PORT=4022;;
        *) echo "could not auto-detect /health port for family=$FAMILY" >&2; exit 65;;
    esac
}

current_sha() {
    docker inspect --format='{{index .Config.Labels "x402.oracle.sha"}}' \
        "${IMAGE}:current" 2>/dev/null || true
}

retag_current_as_previous() {
    if docker image inspect "${IMAGE}:current" >/dev/null 2>&1; then
        docker tag "${IMAGE}:current" "${IMAGE}:previous"
        local cur
        cur="$(current_sha)"
        if [[ -n "$cur" ]]; then
            echo "[deploy] saved ${IMAGE}:current → :previous (was sha=${cur})"
        else
            echo "[deploy] saved ${IMAGE}:current → :previous"
        fi
    fi
}

probe_health() {
    local deadline=$((SECONDS + HEALTH_TIMEOUT))
    while (( SECONDS < deadline )); do
        if curl -fsS "http://127.0.0.1:${HEALTH_PORT}/health" 2>/dev/null \
                | jq -e '.status == "healthy"' >/dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    return 1
}

# After a successful loopback /health probe, warn the operator if the
# binary is bound to 127.0.0.1 only. External clients (other oracles, the
# spl-token-balance handler, buyers running the seller-register flow) will
# get "Empty reply from server" until either:
#   1. BIND_ADDR is changed to 0.0.0.0:<port> (Posture A — direct bind), or
#   2. nginx / Caddy / Cloudflare is fronting the loopback port (Posture B).
#
# This warning is silent when BIND_ADDR=0.0.0.0 (already exposed) or when
# we couldn't parse BIND_ADDR (operator overrode by other means).
warn_if_loopback_only() {
    [[ "$BIND_INTERFACE" == "127.0.0.1" ]] || return 0
    cat <<EOF >&2

⚠  POST-DEPLOY REACHABILITY WARNING
   ${SERVICE} is bound to 127.0.0.1:${HEALTH_PORT} (loopback only).
   External clients cannot reach the registry until you do ONE of:

   (A) Edit ${ENV_FILE}: change BIND_ADDR to 0.0.0.0:${HEALTH_PORT}, then
       restart the unit. Plain HTTP — fine for devnet.

   (B) Front the loopback port with nginx / Caddy / Cloudflare for TLS
       termination, then expose :443 only. Required for mainnet.

   See scripts/docker/${FAMILY}-${CLUSTER}.env.example for the documented
   Posture A / Posture B comment block.

EOF
}

# ----- Sanity checks --------------------------------------------------------

require_tool docker
require_tool curl
require_tool jq

if ! systemctl list-unit-files "${SERVICE}" --no-legend 2>/dev/null | grep -q "^${SERVICE}"; then
    echo "systemd unit not installed: ${SERVICE}" >&2
    echo "expected file at /etc/systemd/system/${SERVICE}" >&2
    echo "see scripts/docker/README.md for one-time setup" >&2
    exit 65
fi
if [[ ! -f "$ENV_FILE" ]]; then
    echo "env file not found: ${ENV_FILE}" >&2
    echo "see scripts/docker/${FAMILY}-${CLUSTER}.env.example" >&2
    exit 65
fi

detect_health_port

# ----- Rollback path --------------------------------------------------------
if (( ROLLBACK )); then
    require_root
    if ! docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
        echo "no ${IMAGE}:previous image to roll back to" >&2
        exit 65
    fi
    echo "[rollback] restoring ${IMAGE}:previous → :current"
    docker tag "${IMAGE}:previous" "${IMAGE}:current"
    systemctl restart "${SERVICE}"
    if probe_health; then
        echo "[rollback] /health → healthy on port ${HEALTH_PORT}"
        warn_if_loopback_only
        exit 0
    fi
    echo "[rollback] /health did not flip to healthy within ${HEALTH_TIMEOUT}s" >&2
    exit 1
fi

# ----- Build path -----------------------------------------------------------
require_root

SHA="$(git -C "$WORKSPACE_ROOT" rev-parse --short=12 HEAD 2>/dev/null || true)"
if [[ -z "$SHA" ]]; then
    echo "could not resolve git short-SHA from ${WORKSPACE_ROOT}" >&2
    echo "the workspace must be a git checkout for SHA-based image tagging" >&2
    exit 65
fi
IMAGE_SHA="${IMAGE}:${SHA}"

if (( SKIP_BUILD )); then
    if ! docker image inspect "${IMAGE_SHA}" >/dev/null 2>&1; then
        echo "--skip-build set but ${IMAGE_SHA} is not present locally" >&2
        echo "run without --skip-build to rebuild, or 'docker load' an image" >&2
        exit 65
    fi
    echo "[deploy] reusing existing image ${IMAGE_SHA}"
else
    echo "[deploy] building ${IMAGE_SHA} from ${WORKSPACE_ROOT} (cluster=${CLUSTER}, features=${CARGO_FEATURES:-none})"
    DOCKER_BUILDKIT=1 docker build \
        --network host \
        -f "${WORKSPACE_ROOT}/scripts/docker/Dockerfile" \
        --build-arg "FAMILY=${FAMILY}" \
        --build-arg "CARGO_FEATURES=${CARGO_FEATURES}" \
        --label "x402.oracle.family=${FAMILY}" \
        --label "x402.oracle.cluster=${CLUSTER}" \
        --label "x402.oracle.sha=${SHA}" \
        -t "${IMAGE_SHA}" \
        "${WORKSPACE_ROOT}"
fi

# Save the existing :current → :previous so --rollback has a target.
retag_current_as_previous

# Promote the new SHA → :current and restart the unit.
docker tag "${IMAGE_SHA}" "${IMAGE}:current"
echo "[deploy] tagged ${IMAGE_SHA} → ${IMAGE}:current"

systemctl restart "${SERVICE}"
echo "[deploy] restarted ${SERVICE}; probing /health on port ${HEALTH_PORT}…"

if probe_health; then
    echo "[deploy] /health → healthy"
    warn_if_loopback_only
    echo "[deploy] done. To roll back: sudo bash $0 --unit ${UNIT} --rollback"
    exit 0
fi

echo "[deploy] /health did not flip to healthy within ${HEALTH_TIMEOUT}s" >&2
echo "[deploy] auto-rolling back to :previous (if available)…" >&2
if docker image inspect "${IMAGE}:previous" >/dev/null 2>&1; then
    docker tag "${IMAGE}:previous" "${IMAGE}:current"
    systemctl restart "${SERVICE}"
    if probe_health; then
        echo "[deploy] rolled back; /health → healthy" >&2
        warn_if_loopback_only
        exit 1
    fi
    echo "[deploy] rollback also failed; manual intervention required" >&2
    exit 2
fi
echo "[deploy] no :previous image; manual intervention required" >&2
exit 2
