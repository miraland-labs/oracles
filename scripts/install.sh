#!/usr/bin/env bash
# Install one x402 oracle family on Ubuntu 24.04. Idempotent — safe to re-run.
#
# Usage:
#   sudo ./install.sh <family> <path-to-binary> [env-template]
# Examples:
#   sudo ./install.sh api-quality       ../target/release/oracle-api-quality
#   sudo ./install.sh onchain-transfer  ../target/release/oracle-onchain-transfer
#   sudo ./install.sh file-delivery     ../target/release/oracle-file-delivery
#
# After install, edit /etc/oracle/<family>.env and `systemctl restart oracle@<family>.service`.

set -euo pipefail

FAMILY="${1:?family name required, e.g. api-quality, onchain-transfer, file-delivery}"
BINARY="${2:?path to compiled binary}"
ENV_TEMPLATE="${3:-./.env.example}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 1. system user / group
if ! id -u oracle >/dev/null 2>&1; then
  useradd --system --home /var/lib/oracle --shell /usr/sbin/nologin oracle
fi

# 2. directories
install -d -o oracle -g oracle "/opt/oracle/${FAMILY}"
install -d -o oracle -g oracle "/var/lib/oracle/${FAMILY}"
install -d -o root   -g root   -m 0755 /etc/oracle

# 3. binary
install -m 0755 -o oracle -g oracle "${BINARY}" "/opt/oracle/${FAMILY}/oracle-${FAMILY}"

# 4. env file (only if absent — never overwrite an operator's edits)
if [[ ! -f "/etc/oracle/${FAMILY}.env" ]]; then
  install -m 0600 -o oracle -g oracle "${ENV_TEMPLATE}" "/etc/oracle/${FAMILY}.env"
  echo "Wrote /etc/oracle/${FAMILY}.env from template; edit before starting."
fi

# 5. systemd unit + target (templated; written once)
if [[ ! -f /etc/systemd/system/oracle@.service ]]; then
  cp "${SCRIPT_DIR}/oracle@.service" /etc/systemd/system/oracle@.service
fi
if [[ ! -f /etc/systemd/system/oracle.target ]]; then
  cp "${SCRIPT_DIR}/oracle.target" /etc/systemd/system/oracle.target
  systemctl enable oracle.target
fi

# 6. activate
systemctl daemon-reload
systemctl enable --now "oracle@${FAMILY}.service"
systemctl status "oracle@${FAMILY}.service" --no-pager
