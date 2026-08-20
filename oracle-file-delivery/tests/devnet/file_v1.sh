#!/usr/bin/env bash
# End-to-end Devnet runbook for oracle-file-delivery.
#
# What this exercises (Requirement 4.1, 4.3, 6.2, 23.4 + P-FD-* family):
#   1. seller uploads a 5 MiB MP4 blob to MinIO via POST /v1/registry/blob;
#      registry returns sha256 + url
#   2. seller uploads the (small) SLA JSON declaring size + MIME bounds
#   3. buyer funds the SLA-escrow Payment with sla_hash
#   4. seller calls SubmitDelivery(delivery_hash = sha256(blob))
#   5. oracle-file-delivery streams the blob from MinIO, computes SHA-256
#      incrementally, sniffs MIME, and submits ConfirmOracle
#   6. assertions on Payment.resolution_state + oracle_jobs.status
#
# Run from workspace root:
#   bash oracles/oracle-file-delivery/tests/devnet/file_v1.sh

set -euo pipefail

ORACLE_HOST="${ORACLE_HOST:-http://127.0.0.1:4022}"
REGISTRY_BASE="${ORACLE_HOST}/v1/registry"
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"

SLA_FILE="$(mktemp -t sla.XXXXXX.json)"
BLOB_FILE="${BLOB_FILE:-./test-fixtures/example.mp4}"
trap 'rm -f "$SLA_FILE"' EXIT

# 5 MiB minimum, 10 MiB maximum (typical short-clip range).
cat > "$SLA_FILE" <<'JSONEOF'
{
  "version": 1,
  "profile_id": "x402/file-delivery/attestation/v1",
  "listing_id": "550e8400-e29b-41d4-a716-446655440000",
  "expected_size_bytes_min": 5242880,
  "expected_size_bytes_max": 10485760,
  "expected_mime": "video/mp4"
}
JSONEOF

if [[ ! -f "$BLOB_FILE" ]]; then
  cat <<MISSING >&2
BLOB_FILE not found: $BLOB_FILE
Drop a sample MP4 (5..10 MiB) at ./test-fixtures/example.mp4 (or set BLOB_FILE
to its path) before running this runbook.
MISSING
  exit 1
fi

SLA_HASH=$(shasum -a 256 "$SLA_FILE" | awk '{print $1}')
BLOB_HASH=$(shasum -a 256 "$BLOB_FILE" | awk '{print $1}')
BLOB_SIZE=$(stat -f%z "$BLOB_FILE" 2>/dev/null || stat -c%s "$BLOB_FILE")

echo "SLA_HASH=$SLA_HASH"
echo "BLOB_HASH=$BLOB_HASH"
echo "BLOB_SIZE=$BLOB_SIZE"

# 1. Upload SLA (small JSON):
#
# curl -fsS -X POST "$REGISTRY_BASE/sla" \
#     -H "Authorization: Bearer $SELLER_TOKEN" \
#     -H "Content-Type: application/json" \
#     --data-binary "@$SLA_FILE" | jq .
#
# 2. Upload the blob (streamed):
#
# curl -fsS -X POST "$REGISTRY_BASE/blob" \
#     -H "Authorization: Bearer $SELLER_TOKEN" \
#     -H "Content-Type: video/mp4" \
#     --data-binary "@$BLOB_FILE" | jq .
#
# verify: response sha256 == $BLOB_HASH AND size_bytes == $BLOB_SIZE
#
# 3. Buyer funds the escrow with sla_hash and oracle authority for the
#    file-delivery profile.
#
# 4. Seller submits delivery_hash = $BLOB_HASH on-chain.
#
# 5. Oracle streams the blob from MinIO:
#    - 64 KiB read buffer
#    - 512-byte MIME-sniff window (will report video/mp4 from the first ftyp box)
#    - incremental SHA-256, fail-closed on mismatch
#
# 6. Assertions:
#
# psql "$DATABASE_URL" -c "SELECT status, settlement_signature, resolution_hash
#                         FROM oracle_jobs WHERE delivery_hash = '$BLOB_HASH';"

cat <<NOTE
Runbook ready. Configure SELLER_TOKEN, PAYMENT_UID, BUYER, and run through
the numbered steps. For the streaming-fetch path verify that
GET $REGISTRY_BASE/$BLOB_HASH | shasum -a 256
returns $BLOB_HASH (the registry re-verifies before serving — a hash
mismatch surfaces as 500).
NOTE
