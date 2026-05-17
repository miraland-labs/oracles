#!/usr/bin/env bash
# End-to-end Devnet runbook for oracle-api-quality.
#
# Prerequisites:
#   - solana-keygen (or solana CLI 2.x)
#   - jq, curl, shasum
#   - DATABASE_URL pointing at a Postgres with migrations/init.sql applied
#   - The oracle binary running (oracle@api-quality.service or `cargo run`)
#   - oracle-keypair.json funded with Devnet SOL for tx fees
#   - A buyer + seller wallet with Devnet USDC, the seller registered with the
#     registry (POST /v1/registry/seller/register)
#
# What this exercises (Requirement 6.2, 9.1, 11.1):
#   1. seller uploads a hash-bound SLA + delivery JSON to the registry
#   2. buyer funds an SLA-escrow Payment via direct CLI (not pr402, to keep
#      blast radius small)
#   3. seller calls SubmitDelivery on-chain
#   4. oracle-api-quality picks up the event, fetches both artifacts,
#      evaluates, and submits ConfirmOracle
#   5. assertions on the on-chain Payment + the oracle_jobs ledger
#
# Run from the workspace root:
#   bash oracles/oracle-api-quality/tests/devnet/api_quality_v1.sh
#
# This is a doc-style runbook — every step has a `# verify:` comment showing
# the assertion. Production CI should drive this script; for now it's manual.

set -euo pipefail

ORACLE_HOST="${ORACLE_HOST:-http://127.0.0.1:4020}"
REGISTRY_BASE="${ORACLE_HOST}/v1/registry"
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"

SLA_FILE="$(mktemp -t sla.XXXXXX.json)"
DELIVERY_FILE="$(mktemp -t delivery.XXXXXX.json)"
trap 'rm -f "$SLA_FILE" "$DELIVERY_FILE"' EXIT

cat > "$SLA_FILE" <<'JSONEOF'
{
  "version": 1,
  "profile_id": "x402/api-quality/v1",
  "endpoint": "https://seller.example.com/api/premium",
  "method": "GET",
  "required_fields": ["result"],
  "max_latency_ms": 5000,
  "min_status_code": 200,
  "max_status_code": 299
}
JSONEOF

cat > "$DELIVERY_FILE" <<'JSONEOF'
{
  "status_code": 200,
  "latency_ms": 250,
  "response_body": {"result": "ok"},
  "response_headers": {"content-type": "application/json"},
  "timestamp": 1770000000
}
JSONEOF

SLA_HASH=$(shasum -a 256 "$SLA_FILE" | awk '{print $1}')
DELIVERY_HASH=$(shasum -a 256 "$DELIVERY_FILE" | awk '{print $1}')

echo "SLA_HASH=$SLA_HASH"
echo "DELIVERY_HASH=$DELIVERY_HASH"

# ---- 1. Seller uploads SLA + delivery (REQUIRES seller bearer token) ----
#
# Skip if you're driving this against an unauthenticated dev registry; in
# production the bearer is mandatory.
#
# verify: response 200 with sha256 == $SLA_HASH
#
# curl -fsS -X POST "$REGISTRY_BASE/sla" \
#     -H "Authorization: Bearer $SELLER_TOKEN" \
#     -H "Content-Type: application/json" \
#     --data-binary "@$SLA_FILE" | jq .
#
# verify: response 200 with sha256 == $DELIVERY_HASH
#
# curl -fsS -X POST "$REGISTRY_BASE/delivery" \
#     -H "Authorization: Bearer $SELLER_TOKEN" \
#     -H "Content-Type: application/json" \
#     --data-binary "@$DELIVERY_FILE" | jq .

# ---- 2. Buyer funds the escrow ----
#
# Use the SLA-escrow CLI directly (NOT pr402). Replace the placeholders below.
#
# sla-escrow fund-payment \
#     --buyer ./demo-wallets/buyer-keypair.json \
#     --seller "$SELLER_PUBKEY" \
#     --mint  "$DEVNET_USDC_MINT" \
#     --oracle-authority "$ORACLE_PUBKEY" \
#     --payment-uid "$PAYMENT_UID" \
#     --sla-hash "$SLA_HASH" \
#     --amount 1000000 \
#     --ttl 86400
#
# verify: tx confirmed; Payment PDA exists with the snapshotted fields.

# ---- 3. Seller submits delivery ----
#
# sla-escrow submit-delivery \
#     --seller ./demo-wallets/seller-keypair.json \
#     --payment-uid "$PAYMENT_UID" \
#     --delivery-hash "$DELIVERY_HASH"
#
# verify: DeliverySubmittedEvent emitted; oracle WebSocket picks it up.

# ---- 4. Oracle settles ----
#
# Wait up to EVALUATION_TIMEOUT_MS for the oracle to evaluate + settle:
#
# verify: GET /stats shows total_evaluated > 0, total_approved > 0
# verify: oracle_jobs.status = 'settled' for the payment_uid
# verify: oracle_verdicts.approved = true; resolution_hash is 64 hex chars
#
# psql "$DATABASE_URL" -c "SELECT status, settlement_signature, resolution_hash
#                         FROM oracle_jobs WHERE payment_uid = '$PAYMENT_UID_HEX';"
# psql "$DATABASE_URL" -c "SELECT approved, resolution_reason
#                         FROM oracle_verdicts
#                         WHERE oracle_job_id = (
#                             SELECT id FROM oracle_jobs
#                             WHERE payment_uid = '$PAYMENT_UID_HEX'
#                         );"

# ---- 5. Buyer or seller releases the payment ----
#
# sla-escrow release-payment --caller ./demo-wallets/seller-keypair.json \
#     --payment-uid "$PAYMENT_UID"
#
# verify: tokens transferred to seller; on-chain Payment.state = Released.

cat <<'NOTE'

This runbook is structured as a doc with manual verification steps so
operators can drive it on a real Devnet without scripting every CLI call.
The Phase D "final integration milestone" (Task 23) automates these against
a fresh Ubuntu 24.04 VM.

NOTE
