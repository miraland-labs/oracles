#!/usr/bin/env bash
# Upgrade one family binary in place with health probe + optional auto-rollback.
#
# Usage:
#   sudo ./upgrade.sh <family> <path-to-new-binary>
#
# Behavior:
#   1. Captures the running binary as <name>.bak.<timestamp>.
#   2. Stages the new binary as <name>.new (mode 0755, oracle:oracle).
#   3. Atomic-renames .new into place.
#   4. Restarts oracle@<family>.service.
#   5. Probes /health 5x at 2s intervals.
#   6. If --auto-rollback is set, on health failure restores the most recent
#      .bak and restarts; otherwise leaves the binary in place and exits non-zero.
#
# Environment overrides:
#   AUTO_ROLLBACK=1                — same as --auto-rollback flag.
#   PROBE_TIMES=5                  — how many /health probes (default 5).
#   PROBE_DELAY_SECS=2             — seconds between probes (default 2).
#   KEEP_BACKUPS=5                 — backups newer than the Nth most recent are
#                                    kept; older .bak files are pruned (default 5).

set -euo pipefail

FAMILY="${1:?family name required}"
NEW_BINARY="${2:?path to new binary}"
shift 2 || true

AUTO_ROLLBACK="${AUTO_ROLLBACK:-0}"
PROBE_TIMES="${PROBE_TIMES:-5}"
PROBE_DELAY_SECS="${PROBE_DELAY_SECS:-2}"
KEEP_BACKUPS="${KEEP_BACKUPS:-5}"

for arg in "$@"; do
    case "$arg" in
    --auto-rollback) AUTO_ROLLBACK=1 ;;
    *) echo "unknown arg: $arg" >&2; exit 64 ;;
    esac
done

INSTALL_DIR="/opt/oracle/${FAMILY}"
TARGET="${INSTALL_DIR}/oracle-${FAMILY}"
STAGED="${TARGET}.new"
TS="$(date -u +%Y%m%dT%H%M%SZ)"
BACKUP="${TARGET}.bak.${TS}"
ENV_FILE="/etc/oracle/${FAMILY}.env"

if [[ ! -f "${TARGET}" ]]; then
    echo "no running binary at ${TARGET}; run install.sh first" >&2
    exit 1
fi

# 1. Capture the current binary as a timestamped backup.
echo "Capturing backup: ${BACKUP}"
cp -p "${TARGET}" "${BACKUP}"
chown oracle:oracle "${BACKUP}"

# 2. Stage the new binary.
echo "Staging new binary"
install -m 0755 -o oracle -g oracle "${NEW_BINARY}" "${STAGED}"

# 3. Atomic rename into place.
echo "Activating new binary"
mv "${STAGED}" "${TARGET}"

# 4. Restart.
echo "Restarting oracle@${FAMILY}.service"
systemctl restart "oracle@${FAMILY}.service"

# 5. Health probe — read BIND_ADDR's port from the env file (default 4020).
PORT="$(grep -E '^BIND_ADDR=' "${ENV_FILE}" 2>/dev/null | cut -d= -f2- | awk -F: '{print $2}' || true)"
PORT="${PORT:-4020}"

probe_health() {
    local i
    for i in $(seq 1 "${PROBE_TIMES}"); do
        if curl -fsS "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
            echo "oracle-${FAMILY} healthy on port ${PORT} (probe ${i})"
            return 0
        fi
        sleep "${PROBE_DELAY_SECS}"
    done
    return 1
}

if probe_health; then
    # 6a. Healthy — prune old backups.
    if [[ "${KEEP_BACKUPS}" -gt 0 ]]; then
        # shellcheck disable=SC2012  # ls -t sorts by mtime; backups have stable timestamped names
        OLD_BACKUPS=$(ls -1t "${TARGET}".bak.* 2>/dev/null | tail -n +"$((KEEP_BACKUPS + 1))" || true)
        if [[ -n "${OLD_BACKUPS}" ]]; then
            echo "Pruning old backups (keeping newest ${KEEP_BACKUPS}):"
            echo "${OLD_BACKUPS}" | while read -r f; do
                [[ -n "$f" ]] && rm -f "$f" && echo "  removed $f"
            done
        fi
    fi
    exit 0
fi

# 6b. Unhealthy.
echo "oracle-${FAMILY} did NOT come up healthy"
echo "  journalctl -u oracle@${FAMILY}.service --since '1 minute ago'"

if [[ "${AUTO_ROLLBACK}" = "1" ]]; then
    echo "Auto-rollback enabled; restoring ${BACKUP}"
    cp -p "${BACKUP}" "${TARGET}"
    chown oracle:oracle "${TARGET}"
    systemctl restart "oracle@${FAMILY}.service"
    if probe_health; then
        echo "Rollback successful"
        exit 2
    fi
    echo "Rollback FAILED — manual intervention required"
    exit 3
fi

echo "Manual rollback:"
echo "  sudo cp -p ${BACKUP} ${TARGET}"
echo "  sudo systemctl restart oracle@${FAMILY}.service"
exit 1
