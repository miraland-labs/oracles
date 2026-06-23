# 🏹 Buyer Integration Guide (SLA-Escrow)

This guide walks you through funding an escrow and selecting an oracle to secure your service purchase via the `sla-escrow` rail.

---

## 🚀 1. The Core Lifecycle (3 Steps)

### Step A: Pick an Oracle & Profile
Select an oracle profile matching your target service:
* JSON API Quality: `x402/oracles/api-quality/v1`
* On-chain Transfer: `x402/oracles/onchain-transfer/v1`
* File Delivery: `x402/oracles/file-delivery/attestation/v1`

Query the oracle's liveness and policies:
```bash
curl -s "https://oracle.example.com/v1/policy" | jq .
curl -s "https://oracle.example.com/health" | jq .
```

### Step B: Fund the Escrow
Choose one of the two authoring patterns:

#### Path 1: Direct Authoring (You write the SLA)
Construct the SLA JSON, upload it to the registry, get the hash, and build the funding transaction:
```bash
# 1. Author and verify SLA registry upload
SLA_HASH=$(curl -fsS -X POST "https://oracle.example.com/v1/registry/sla" \
    -H "Content-Type: application/json" \
    --data-binary @sla.json | jq -r .sha256)

# 2. Call pr402 to build the FundPayment transaction
UNSIGNED_TX=$(curl -fsS -X POST "https://ipay.sh/api/v1/facilitator/build-sla-escrow-payment-tx" \
    -H "Content-Type: application/json" \
    -d "{\"payer\":\"<your-wallet>\",\"accepted\":{...},\"slaHash\":\"$SLA_HASH\",\"oracleAuthority\":\"<oracle-pubkey>\",\"paymentUidHex\":\"<uid>\"}" | jq -r .transactionBase64)
```

#### Path 2: Delegated Authoring (Seller provides SLA Hash)
If the seller generates the SLA dynamically, they will return the `slaHash` and `paymentUidHex` in their HTTP 402 challenge. You simply feed these pre-computed values directly to the `/build-sla-escrow-payment-tx` endpoint.

### Step C: Claim Refund (If Seller Fails)
If the seller fails to deliver, or the oracle rejects the delivery, you can claw back your funds on-chain once the expiration time (`expires_at`) or refund cooldown has elapsed:

```bash
sla-escrow refund-payment \
    --payment-uid "<payment-uid-hex>" \
    --keypair /path/to/buyer-keypair.json \
    --url devnet
```

---

## 🛡️ Buyer Sanity Checklist

* [ ] **Oracle Liveness:** The chosen oracle's `/health` shows `chain_connected: true`.
* [ ] **Oracle Listed:** The `oracleAuthority` is present in the seller's `accepted.extra.oracleAuthorities[]`.
* [ ] **Profile Match:** The oracle binary supports the profile required by the SLA (e.g. `api-quality/v1`).
* [ ] **SLA Verified:** You verified that the registry contains the SLA bytes matching your purchase intent before sending your payment signature.
* [ ] **Refund Cooldown Checked:** You confirmed the active `refund_cooldown_seconds` on-chain (typically 24 hours).
