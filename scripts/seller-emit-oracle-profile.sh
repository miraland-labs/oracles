#!/usr/bin/env bash
# Emit a ready-to-paste accepts[].extra.oracleProfiles[] entry for an oracle.
#
# A seller advertising the `sla-escrow` rail in their HTTP-402 challenge
# needs to declare which oracle they trust. This helper queries a running
# oracle's /v1/registry/info and /health endpoints (no auth required), then
# prints a single JSON object the seller can paste into their `accepts[].extra`.
#
# Usage:
#   bash seller-emit-oracle-profile.sh <oracle-base-url>
#
# Example:
#   bash seller-emit-oracle-profile.sh https://oracle-api.example.com
#
# Output (printed to stdout, ready to paste):
#   {
#     "profileId": "x402/oracles/api-quality/v1",
#     "operatorPubkey": "OracLe...",
#     "registry": "https://oracle-api.example.com/v1/registry",
#     "normativeSpecUrl": "https://github.com/...",
#     "cluster": "mainnet-beta"            // only for cluster-pinned profiles
#   }
#
# Compose multiple entries by running this script once per oracle and
# wrapping the outputs in a JSON array.
#
# Dependencies: bash, curl, jq.

set -euo pipefail

ORACLE="${1:?usage: seller-emit-oracle-profile.sh <oracle-base-url>}"
ORACLE="${ORACLE%/}"  # strip trailing slash, if any

if ! command -v jq >/dev/null 2>&1; then
    echo "missing dependency: jq" >&2
    exit 64
fi

INFO="$(curl -fsS "${ORACLE}/v1/registry/info")" || {
    echo "failed to fetch ${ORACLE}/v1/registry/info" >&2
    exit 1
}

# Build the profile entry. `cluster` and `normativeSpecUrl` are only emitted
# when present in the oracle's info response (jq's // operator falls through).
echo "${INFO}" | jq \
    --arg registry "${ORACLE}/v1/registry" \
    '{
        profileId: .registeredProfileId,
        operatorPubkey: .oraclePubkey,
        registry: $registry
    }
    + (if .normativeSpecUrl  then {normativeSpecUrl:  .normativeSpecUrl}  else {} end)
    + (if .cluster           then {cluster:           .cluster}           else {} end)'
