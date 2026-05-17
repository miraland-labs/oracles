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
# Dependencies: bash, curl, jq, solana CLI (>=1.18 / 2.x).

set -euo pipefail

ORACLE="${1:?usage: seller-register.sh <oracle-base-url> <keypair-path> [label]}"
KEYPAIR="${2:?usage: seller-register.sh <oracle-base-url> <keypair-path> [label]}"
LABEL="${3:-seller-key-$(date -u +%Y%m%dT%H%M%SZ)}"

if ! command -v jq >/dev/null 2>&1; then
    echo "missing dependency: jq" >&2
    exit 64
fi
if ! command -v solana >/dev/null 2>&1; then
    echo "missing dependency: solana CLI" >&2
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

# 2. Sign the challenge bytes with the seller keypair.
#
# `solana sign-offchain-message` needs a --wallet-only signer (this keypair).
# Older CLIs lack the --raw flag; we fall back to a Python helper if needed.
SIGNATURE=""
if solana sign-offchain-message --help 2>/dev/null | grep -q -- '--keypair'; then
    SIGNATURE="$(solana sign-offchain-message \
        --keypair "${KEYPAIR}" \
        "${CHALLENGE}" 2>/dev/null || true)"
fi

if [[ -z "${SIGNATURE}" ]]; then
    # Fallback: use Python + nacl. We try to keep this rare so seller users
    # only need the Solana CLI.
    if ! command -v python3 >/dev/null 2>&1; then
        echo "could not sign with solana CLI and python3 not available; install" >&2
        echo "a recent solana CLI (>=1.18) or python3 with pynacl + base58" >&2
        exit 1
    fi
    SIGNATURE="$(python3 - <<PYEOF
import base58, json, sys
try:
    import nacl.signing
except Exception as e:
    print(f"missing pynacl ({e}); pip install pynacl base58", file=sys.stderr)
    sys.exit(1)

with open("${KEYPAIR}") as f:
    raw = json.load(f)
secret = bytes(raw)[:32]
sk = nacl.signing.SigningKey(secret)
sig = sk.sign(b"${CHALLENGE}").signature
print(base58.b58encode(sig).decode("ascii"))
PYEOF
    )"
fi

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
