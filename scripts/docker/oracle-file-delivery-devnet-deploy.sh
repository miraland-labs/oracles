#!/usr/bin/env bash
# Rebuild + restart + health-probe for preview oracle-file-delivery.
# Thin wrapper around oracle-deploy.sh so the unit name cannot be mistyped
# onto oracle-onchain-transfer-*.
#
# Usage (from the oracles checkout, as root):
#   sudo bash scripts/docker/oracle-file-delivery-devnet-deploy.sh
#   sudo bash scripts/docker/oracle-file-delivery-devnet-deploy.sh --rollback
#   sudo bash scripts/docker/oracle-file-delivery-devnet-deploy.sh --skip-build
#
# First time on the box: run oracle-file-delivery-devnet-setup.sh instead.

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec bash "${DIR}/oracle-deploy.sh" --unit oracle-file-delivery-devnet "$@"
