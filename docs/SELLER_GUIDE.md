# 📦 Seller Integration Guide (SLA-Escrow)

This guide walks you through integrating your API or service with the `sla-escrow` rail. 

In an escrow transaction, the buyer locks funds on-chain, you perform the work, upload evidence to the oracle's registry, and commit the delivery hash on-chain. A designated oracle then verifies your delivery and releases the funds.

---

## 🚀 1. The Core Lifecycle (3 Steps)

### Step A: Authenticate with the Oracle (One-time)
Register your merchant wallet with the chosen oracle to receive a bearer token for registry uploads.

```bash
# Register using your Solana wallet keypair
bash oracles/scripts/seller-register.sh \
    https://oracle.example.com \
    /path/to/seller-keypair.json
# → Saves bearer token to $SELLER_TOKEN
```

### Step B: Upload SLA & Do Work
Upload the SLA contract JSON to the oracle registry, retrieve the SHA-256 hash, and do the work.

```bash
# Upload SLA JSON
SLA_HASH=$(curl -fsS -X POST "https://oracle.example.com/v1/registry/sla" \
    -H "Authorization: Bearer $SELLER_TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)
```

### Step C: Upload Evidence & Submit Delivery
After doing the work, construct your evidence JSON, upload it, and announce completion on-chain.

```bash
# 1. Upload Evidence JSON
DELIVERY_HASH=$(curl -fsS -X POST "https://oracle.example.com/v1/registry/delivery" \
    -H "Authorization: Bearer $SELLER_TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary @delivery.json | jq -r .sha256)

# 2. Submit Delivery on-chain
sla-escrow submit-delivery \
    --seller /path/to/seller-keypair.json \
    --payment-uid "<payment-uid-hex>" \
    --delivery-hash "$DELIVERY_HASH"
```

---

## 🛠️ 2. Profile-Specific Recipes

### Profile A: JSON HTTP API (`x402/oracles/api-quality/v1`)
Upload details about your HTTP response checks.
* **SLA JSON:** Contains `endpoint`, `method`, `min_status_code`, and `max_latency_ms`.
* **Evidence JSON:** Contains `status_code`, `latency_ms`, and `response_body`.

### Profile B: On-Chain SPL Transfer (`x402/oracles/onchain-transfer/v1`)
Use this when you deliver tokens on-chain to the buyer.
* **SLA JSON:** Contains `recipient_owner`, `mint`, and `min_amount`.
* **Evidence JSON:** Contains `tx_signature` of the completed transfer. The oracle verifies the transfer directly via RPC.

### Profile C: Large File Delivery (`x402/oracles/file-delivery/attestation/v1`)
Use this when delivering raw assets (e.g. video files).
* **SLA JSON:** Contains `expected_mime` and `min_bytes`.
* **Evidence Upload:** Post the raw binary directly to the registry:
  ```bash
  DELIVERY_HASH=$(curl -fsS -X POST "https://oracle.example.com/v1/registry/blob" \
      -H "Authorization: Bearer $SELLER_TOKEN" \
      -H "Content-Type: video/mp4" \
      --data-binary @output.mp4 | jq -r .sha256)
  ```

---

## 🛡️ Seller Sanity Checklist

* [ ] **Bearer Token Saved:** You registered and saved `$SELLER_TOKEN`.
* [ ] **Exact Hash Matching:** You submit the *exact* `sha256` returned by the oracle registry to the blockchain. Do not recalculate hashes locally.
* [ ] **Freshness Enforcement:** Your evidence timestamp is *after* the payment creation time (prevents old transaction replay).
* [ ] **Timely Delivery:** You submit the transaction before the escrow payment `expires_at` deadline minus the oracle's safety buffer.
