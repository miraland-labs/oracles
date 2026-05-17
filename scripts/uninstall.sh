#!/usr/bin/env bash
# Uninstall one family binary. Preserves the env file by default
# (set PRESERVE_ENV=0 to remove it too).
#
# Usage:
#   sudo ./uninstall.sh <family>
#   sudo PRESERVE_ENV=0 ./uninstall.sh <family>

set -euo pipefail

FAMILY="${1:?family name required}"
PRESERVE_ENV="${PRESERVE_ENV:-1}"

systemctl disable --now "oracle@${FAMILY}.service" || true
rm -f "/opt/oracle/${FAMILY}/oracle-${FAMILY}"
rmdir "/opt/oracle/${FAMILY}" 2>/dev/null || true
rmdir "/var/lib/oracle/${FAMILY}" 2>/dev/null || true

if [[ "${PRESERVE_ENV}" != "1" ]]; then
  rm -f "/etc/oracle/${FAMILY}.env"
fi

echo "Removed oracle-${FAMILY} (env preserved=${PRESERVE_ENV})"
