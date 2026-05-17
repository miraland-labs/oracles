#!/usr/bin/env bash
# Print the SQL an operator runs to register a freshly-deployed oracle with
# pr402's `parameters` table so it gets advertised on `GET /capabilities`
# under `slaEscrowOracleProfiles[]`.
#
# This script is **read-only** — it queries the running oracle's
# /v1/registry/info and /health endpoints (no auth) and prints the exact
# `psql` snippet for the operator to run against pr402's database. It does
# NOT connect to pr402. Production-safe by design: no bearer needed, no
# write access required, no risk of invoking a wrong endpoint by accident.
#
# Usage:
#   bash announce-to-pr402.sh <oracle-base-url>
#
# Example:
#   bash announce-to-pr402.sh https://oracle-api.example.com
#
# Output: a short copy-paste-ready `psql` block. Hand it to whoever owns
# pr402's deployment (or run it yourself if you operate both).
#
# Why not auto-write? pr402 is a production service shared across multiple
# integrators. Direct write access is intentionally restricted to operators
# who know what they're doing. Generating the SQL keeps the human in the
# loop without making them hand-craft INSERT statements.
#
# Dependencies: bash, curl, jq.

set -euo pipefail

ORACLE="${1:?usage: announce-to-pr402.sh <oracle-base-url>}"
ORACLE="${ORACLE%/}"

if ! command -v jq >/dev/null 2>&1; then
    echo "missing dependency: jq" >&2
    exit 64
fi

INFO="$(curl -fsS "${ORACLE}/v1/registry/info")" || {
    echo "failed to fetch ${ORACLE}/v1/registry/info" >&2
    exit 1
}

PROFILE_ID="$(echo "${INFO}" | jq -r .registeredProfileId)"
PUBKEY="$(echo "${INFO}" | jq -r .oraclePubkey)"
SPEC_URL="$(echo "${INFO}" | jq -r '.normativeSpecUrl // ""')"
CLUSTER="$(echo "${INFO}" | jq -r '.cluster // ""')"
REGISTRY="${ORACLE}/v1/registry"

# Map canonical profileId → ergonomic per-profile parameter key prefix.
# These prefixes are read by pr402's /capabilities discovery handler.
case "${PROFILE_ID}" in
    "x402/oracles/api-quality/v1")
        KEY_PREFIX="PR402_SLA_ESCROW_API_QUALITY"
        ;;
    "x402/oracles/onchain-transfer/v1")
        KEY_PREFIX="PR402_SLA_ESCROW_ONCHAIN_TRANSFER"
        ;;
    "x402/oracles/file-delivery/attestation/v1")
        KEY_PREFIX="PR402_SLA_ESCROW_FILE_DELIVERY"
        ;;
    *)
        echo "Unknown profileId: ${PROFILE_ID}" >&2
        echo "For custom profiles, set PR402_SLA_ESCROW_ORACLE_PROFILES_JSON via the parameters table directly." >&2
        exit 1
        ;;
esac

cat <<SQL
-- Register oracle ${PUBKEY}
-- Profile:  ${PROFILE_ID}
-- Registry: ${REGISTRY}
--
-- Run this against pr402's Postgres (the one whose URL is in pr402's
-- DATABASE_URL). The cache TTL is 60s by default — /capabilities will pick
-- up the new entry within a minute, no restart required.

INSERT INTO parameters (param_name, param_value) VALUES
    ('${KEY_PREFIX}_DEFAULT_PUBKEY',         '${PUBKEY}'),
    ('${KEY_PREFIX}_REGISTRY_URL',           '${REGISTRY}')$([ -n "${SPEC_URL}" ] && echo ",
    ('${KEY_PREFIX}_NORMATIVE_SPEC_URL',     '${SPEC_URL}')")
ON CONFLICT (param_name) DO UPDATE SET
    param_value = EXCLUDED.param_value,
    updated_at = NOW();
SQL

if [[ -n "${CLUSTER}" ]]; then
    cat <<NOTE

-- Cluster pinning detected: ${CLUSTER}
-- Confirm pr402's chainId at GET /api/v1/facilitator/health matches before
-- accepting traffic against this oracle (sellers/buyers will hit
-- Custom(258) ClusterMismatch otherwise).
NOTE
fi

cat <<VERIFY

-- Verify the announcement landed (after ~60s):
--
--   curl -fsS https://<your-pr402-host>/api/v1/facilitator/capabilities \\
--       | jq '.slaEscrowOracleProfiles[] | select(.profileId=="${PROFILE_ID}")'
VERIFY
