#!/usr/bin/env bash
# Minimal seller-side helper: sign the oracle's challenge with a Solana
# wallet keypair and exchange it for a bearer token.
#
# Usage:
#   bash seller-register.sh <oracle-base-url> <keypair-path> [label]
#
# Example:
#   bash seller-register.sh https://oracle-api.example.com ./seller.json my-prod-key
#
# Prints (to stdout):
#   BEARER=<long-base58-token>
#
# Save the printed bearer; the oracle stores only SHA256(token) and never
# returns it again. If you lose it, run /v1/registry/seller/rotate.
#
# Dependencies: bash, curl, jq, python3 + pynacl + base58.
#
# We deliberately do NOT use `solana sign-offchain-message`: the Solana CLI
# wraps the message in the SIMD-0048 offchain envelope before signing, which
# the oracle's seller/register handler does not verify against. The oracle
# checks the signature over the raw challenge bytes; only a direct ed25519
# sign produces the right shape. Hence the Python path is the canonical
# implementation.

set -euo pipefail

ORACLE="${1:?usage: seller-register.sh <oracle-base-url> <keypair-path> [label]}"
KEYPAIR="${2:?usage: seller-register.sh <oracle-base-url> <keypair-path> [label]}"
LABEL="${3:-seller-key-$(date -u +%Y%m%dT%H%M%SZ)}"

if ! command -v jq >/dev/null 2>&1; then
    echo "missing dependency: jq" >&2
    exit 64
fi
if ! command -v solana-keygen >/dev/null 2>&1; then
    echo "missing dependency: solana-keygen (used to derive the wallet pubkey)" >&2
    exit 64
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "missing dependency: python3 (used to sign the challenge with raw ed25519)" >&2
    exit 64
fi
if [[ ! -f "${KEYPAIR}" ]]; then
    echo "keypair not found: ${KEYPAIR}" >&2
    exit 1
fi

WALLET="$(solana-keygen pubkey "${KEYPAIR}")"

# 1. Challenge.
CHALL_JSON="$(curl -fsS "${ORACLE}/v1/registry/seller/challenge?wallet=${WALLET}")"
CHALLENGE="$(echo "${CHALL_JSON}" | jq -r .challenge)"
if [[ -z "${CHALLENGE}" || "${CHALLENGE}" == "null" ]]; then
    echo "challenge endpoint returned no challenge:" >&2
    echo "${CHALL_JSON}" >&2
    exit 1
fi

# 2. Sign the raw challenge bytes with ed25519.
#
# The oracle verifies the signature over the raw challenge string (no
# envelope, no framing). `solana sign-offchain-message` would NOT work here
# because it signs the SIMD-0048 envelope. We use python3 + pynacl which
# performs a direct ed25519 sign over the challenge bytes.
SIGNATURE="$(KEYPAIR_PATH="${KEYPAIR}" CHALLENGE="${CHALLENGE}" python3 - <<'PYEOF'
import json
import os
import sys

try:
    import base58
    import nacl.signing
except ImportError as e:
    print(f"missing python dependency ({e}); install with: pip3 install pynacl base58", file=sys.stderr)
    sys.exit(1)

with open(os.environ["KEYPAIR_PATH"]) as f:
    raw = json.load(f)
secret = bytes(raw)[:32]
sk = nacl.signing.SigningKey(secret)
sig = sk.sign(os.environ["CHALLENGE"].encode("ascii")).signature
print(base58.b58encode(sig).decode("ascii"))
PYEOF
)"

if [[ -z "${SIGNATURE}" ]]; then
    echo "failed to sign challenge" >&2
    exit 1
fi

# 3. Register.
REG_JSON="$(curl -fsS -X POST "${ORACLE}/v1/registry/seller/register" \
    -H "Content-Type: application/json" \
    -d "{
        \"wallet\": \"${WALLET}\",
        \"signature\": \"${SIGNATURE}\",
        \"challenge\": \"${CHALLENGE}\",
        \"label\": \"${LABEL}\"
    }")"

TOKEN="$(echo "${REG_JSON}" | jq -r .token)"
if [[ -z "${TOKEN}" || "${TOKEN}" == "null" ]]; then
    echo "register endpoint returned no token:" >&2
    echo "${REG_JSON}" >&2
    exit 1
fi

echo "BEARER=${TOKEN}"
