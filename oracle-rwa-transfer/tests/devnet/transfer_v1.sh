#!/usr/bin/env bash
# End-to-end Devnet runbook for oracle-rwa-transfer.
#
# What this exercises (Requirement 6.2, 9.1, 23.4 + P-OT-* family):
#   1. seller broadcasts a real TransferChecked of devnet USDC
#      to a buyer wallet, captures the signature
#   2. seller uploads the (small) evidence JSON listing tx_signature +
#      asserted_transfers to the registry
#   3. buyer funds the SLA-escrow Payment with the corresponding sla_hash
#   4. seller calls SubmitDelivery(delivery_hash)
#   5. oracle-rwa-transfer fetches the SLA + evidence, calls
#      getTransaction(sig, jsonParsed), re-derives pre/post token deltas,
#      and submits ConfirmOracle
#   6. assertions on Payment.resolution_state + oracle_jobs.status
#
# Run from workspace root:
#   bash oracles/oracle-rwa-transfer/tests/devnet/transfer_v1.sh

set -euo pipefail

ORACLE_HOST="${ORACLE_HOST:-http://127.0.0.1:4021}"
REGISTRY_BASE="${ORACLE_HOST}/v1/registry"
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
TRANSFER_CLUSTER="${TRANSFER_CLUSTER:-devnet}"

SLA_FILE="$(mktemp -t sla.XXXXXX.json)"
DELIVERY_FILE="$(mktemp -t delivery.XXXXXX.json)"
trap 'rm -f "$SLA_FILE" "$DELIVERY_FILE"' EXIT

# Replace these placeholders before driving the runbook.
DEVNET_USDC_MINT="${DEVNET_USDC_MINT:-Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB}"
BUYER_PUBKEY="${BUYER_PUBKEY:?set BUYER_PUBKEY}"
TX_SIGNATURE="${TX_SIGNATURE:?set TX_SIGNATURE — the transfer signature the seller broadcast}"

# Build SLA: expect at least 1_000_000 raw units of USDC INTO the buyer wallet.
cat > "$SLA_FILE" <<JSONEOF
{
  "version": 1,
  "profile_id": "x402/rwa-transfer/v1",
  "cluster": "${TRANSFER_CLUSTER}",
  "expected_transfers": [
    {
      "mint": "${DEVNET_USDC_MINT}",
      "recipient_owner": "${BUYER_PUBKEY}",
      "min_amount": "1000000",
      "direction": "in"
    }
  ]
}
JSONEOF

cat > "$DELIVERY_FILE" <<JSONEOF
{
  "version": 1,
  "profile_id": "x402/rwa-transfer/v1",
  "tx_signature": "${TX_SIGNATURE}",
  "asserted_transfers": [
    {
      "mint": "${DEVNET_USDC_MINT}",
      "recipient_owner": "${BUYER_PUBKEY}",
      "claimed_delta": "1000000"
    }
  ],
  "submitted_at": $(date +%s)
}
JSONEOF

SLA_HASH=$(shasum -a 256 "$SLA_FILE" | awk '{print $1}')
DELIVERY_HASH=$(shasum -a 256 "$DELIVERY_FILE" | awk '{print $1}')

echo "SLA_HASH=$SLA_HASH"
echo "DELIVERY_HASH=$DELIVERY_HASH"

# 1. Seller uploads SLA + delivery (steps marked here are doc-only; uncomment
#    when the seller bearer is configured).
# 2. Buyer funds the escrow with sla_hash.
# 3. Seller submits delivery on-chain with delivery_hash.
# 4. Oracle:
#    - fetches SLA + evidence
#    - calls getTransaction(TX_SIGNATURE, jsonParsed)
#    - re-derives delta = post.amount - pre.amount for (mint, owner)
#    - approves iff delta >= min_amount with direction match
# 5. Assertions:
#    - oracle_jobs.status = 'settled'
#    - oracle_verdicts.approved = true (or false with the reason in [256..263])
#
# psql "$DATABASE_URL" -c "SELECT status, last_error, resolution_hash
#                         FROM oracle_jobs WHERE delivery_hash = '$DELIVERY_HASH';"
# psql "$DATABASE_URL" -c "SELECT approved, resolution_reason
#                         FROM oracle_verdicts
#                         WHERE oracle_job_id = (
#                             SELECT id FROM oracle_jobs
#                             WHERE delivery_hash = '$DELIVERY_HASH'
#                         );"

cat <<NOTE
Runbook ready. Configure SELLER_TOKEN, PAYMENT_UID, BUYER_PUBKEY, and
TX_SIGNATURE then walk through the numbered steps. The full integration
test (Phase D Task 23) wraps this in a single command-line runner.
NOTE
