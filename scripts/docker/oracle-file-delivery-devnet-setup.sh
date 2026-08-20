#!/usr/bin/env bash
# One-time (idempotent) host prep + first deploy of oracle-file-delivery
# against Solana DEVNET / Forge preview, on a box that already runs
# Dockerized oracle-onchain-transfer.
#
# Does NOT start, stop, rebuild, or rewrite:
#   oracle-onchain-transfer-devnet / -mainnet
#   ports 4021 / 4031
#   databases oracle_onchain_transfer_*
#   keypairs under /var/lib/oracle/onchain-transfer-*
#
# Usage (from the oracles checkout):
#   sudo bash scripts/docker/oracle-file-delivery-devnet-setup.sh \
#       --keypair /path/to/oracle-keypair.json
#
# Flags:
#   --keypair <path>      Solana keypair JSON to install as this family's
#                         oracle_authority (required unless the dest file
#                         already exists). Do not pass an onchain-transfer key.
#   --database-url <url>  Override DATABASE_URL. Default: reuse oracle_app
#                         user/password from /etc/oracle/onchain-transfer-devnet.env
#                         with database name oracle_file_delivery_devnet.
#   --skip-deploy         Prep unit + env + db + keypair only; do not build.
#   --skip-build          Prep, then oracle-deploy.sh --skip-build.

set -euo pipefail

KEYPAIR_SRC=""
DATABASE_URL_OVERRIDE=""
SKIP_DEPLOY=0
SKIP_BUILD=0

UNIT="oracle-file-delivery-devnet"
FAMILY="file-delivery"
CLUSTER="devnet"
BIND_PORT=4022
DB_NAME="oracle_file_delivery_devnet"
ENV_FILE="/etc/oracle/file-delivery-devnet.env"
KEYPAIR_DEST="/var/lib/oracle/file-delivery-devnet/oracle-keypair.json"
SIBLING_ENV="/etc/oracle/onchain-transfer-devnet.env"
TOKEN_NOTE="/root/oracle-file-delivery-devnet.operator-token"

RESERVED_UNITS=(
    oracle-onchain-transfer-devnet.service
    oracle-onchain-transfer-mainnet.service
)
RESERVED_CONTAINERS=(
    oracle-onchain-transfer-devnet
    oracle-onchain-transfer-mainnet
)
RESERVED_PORTS=(4021 4031)
RESERVED_KEYPAIR_DIRS=(
    /var/lib/oracle/onchain-transfer-devnet
    /var/lib/oracle/onchain-transfer-mainnet
)

while [[ $# -gt 0 ]]; do
    case "$1" in
        --keypair)       KEYPAIR_SRC="$2"; shift 2;;
        --keypair=*)     KEYPAIR_SRC="${1#*=}"; shift;;
        --database-url)  DATABASE_URL_OVERRIDE="$2"; shift 2;;
        --database-url=*) DATABASE_URL_OVERRIDE="${1#*=}"; shift;;
        --skip-deploy)   SKIP_DEPLOY=1; shift;;
        --skip-build)    SKIP_BUILD=1; shift;;
        -h|--help)
            sed -n '2,$ s/^# \{0,1\}//p' "$0" | head -28
            exit 0;;
        *) echo "unknown arg: $1" >&2; exit 64;;
    esac
done

if [[ $EUID -ne 0 ]]; then
    echo "must run as root (uses systemctl + docker + postgres)" >&2
    exit 77
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT=""
if command -v git >/dev/null 2>&1; then
    WORKSPACE_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
fi
if [[ -z "$WORKSPACE_ROOT" ]]; then
    WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
fi
if [[ ! -f "${WORKSPACE_ROOT}/Cargo.toml" ]]; then
    echo "WORKSPACE_ROOT=${WORKSPACE_ROOT} does not contain Cargo.toml" >&2
    exit 65
fi

UNIT_SRC="${SCRIPT_DIR}/${UNIT}.service"
ENV_SRC="${SCRIPT_DIR}/file-delivery-devnet.env.example"
INIT_SQL="${WORKSPACE_ROOT}/oracle-common/migrations/init.sql"

for f in "$UNIT_SRC" "$ENV_SRC" "$INIT_SQL"; do
    if [[ ! -f "$f" ]]; then
        echo "missing file: $f" >&2
        exit 65
    fi
done

require_tool() {
    command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 65; }
}

require_tool docker
require_tool curl
require_tool jq
require_tool psql
require_tool openssl

# ----- Refuse to collide with onchain-transfer --------------------------------

for u in "${RESERVED_UNITS[@]}"; do
    if [[ ! -f "/etc/systemd/system/${u}" ]]; then
        echo "[setup] note: sibling unit ${u} is not installed (ok if this host only has one cluster)"
    fi
done

for name in "${RESERVED_CONTAINERS[@]}"; do
    if docker ps -a --format '{{.Names}}' | grep -qx "$name"; then
        echo "[setup] sibling container ${name} present — will not stop or recreate it"
    fi
done

port_in_use() {
    local port="$1"
    if command -v ss >/dev/null 2>&1; then
        ss -ltn | awk '{print $4}' | grep -Eq ":${port}$"
        return $?
    fi
    return 1
}

for p in "${RESERVED_PORTS[@]}"; do
    if port_in_use "$p"; then
        echo "[setup] port ${p} is in use (expected: onchain-transfer). leaving it alone."
    fi
done

if port_in_use "$BIND_PORT"; then
    if docker ps --format '{{.Names}}' | grep -qx "$UNIT"; then
        echo "[setup] port ${BIND_PORT} already owned by ${UNIT}"
    else
        echo "port ${BIND_PORT} is in use by something other than ${UNIT}" >&2
        echo "file-delivery preview binds ${BIND_PORT}; 4021/4031 are reserved for onchain-transfer" >&2
        exit 65
    fi
fi

# ----- Keypair ----------------------------------------------------------------

install -d -o root -g root -m 0750 /var/lib/oracle/file-delivery-devnet
install -d -o root -g root -m 0750 /etc/oracle

if [[ -n "$KEYPAIR_SRC" ]]; then
    if [[ ! -f "$KEYPAIR_SRC" ]]; then
        echo "keypair not found: $KEYPAIR_SRC" >&2
        exit 65
    fi
    abs_src="$(cd "$(dirname "$KEYPAIR_SRC")" && pwd)/$(basename "$KEYPAIR_SRC")"
    for d in "${RESERVED_KEYPAIR_DIRS[@]}"; do
        if [[ "$abs_src" == "${d}/oracle-keypair.json" ]]; then
            echo "refusing to install an onchain-transfer keypair as file-delivery authority" >&2
            echo "each family needs its own oracle_authority" >&2
            exit 65
        fi
    done
    install -m 0600 -o root -g root "$KEYPAIR_SRC" "$KEYPAIR_DEST"
    echo "[setup] installed keypair → ${KEYPAIR_DEST}"
elif [[ -f "$KEYPAIR_DEST" ]]; then
    echo "[setup] reusing existing ${KEYPAIR_DEST}"
else
    echo "no keypair at ${KEYPAIR_DEST}; pass --keypair /path/to/oracle-keypair.json" >&2
    exit 65
fi

if command -v solana-keygen >/dev/null 2>&1; then
    PUBKEY="$(solana-keygen pubkey "$KEYPAIR_DEST")"
    echo "[setup] file-delivery-devnet oracle pubkey: ${PUBKEY}"
    for d in "${RESERVED_KEYPAIR_DIRS[@]}"; do
        sibling="${d}/oracle-keypair.json"
        if [[ -f "$sibling" ]]; then
            sib_pk="$(solana-keygen pubkey "$sibling" 2>/dev/null || true)"
            if [[ -n "$sib_pk" && "$sib_pk" == "$PUBKEY" ]]; then
                echo "file-delivery keypair pubkey matches ${sibling}" >&2
                echo "do not share oracle_authority across families" >&2
                exit 65
            fi
        fi
    done
fi

# ----- DATABASE_URL from sibling env ------------------------------------------

parse_pg_url() {
    local url="$1"
    if [[ "$url" =~ ^postgres(ql)?://([^:]+):([^@]+)@([^:/]+):([0-9]+)/([^/?]+) ]]; then
        PG_USER="${BASH_REMATCH[2]}"
        PG_PASS="${BASH_REMATCH[3]}"
        PG_HOST="${BASH_REMATCH[4]}"
        PG_PORT="${BASH_REMATCH[5]}"
        return 0
    fi
    return 1
}

NEW_DATABASE_URL=""
if [[ -n "$DATABASE_URL_OVERRIDE" ]]; then
    NEW_DATABASE_URL="$DATABASE_URL_OVERRIDE"
elif [[ -f "$ENV_FILE" ]]; then
    existing="$(grep -E '^DATABASE_URL=' "$ENV_FILE" | head -1 | cut -d= -f2-)"
    if [[ -n "$existing" && "$existing" != *CHANGE_ME* ]]; then
        NEW_DATABASE_URL="$existing"
        echo "[setup] keeping DATABASE_URL already in ${ENV_FILE}"
    fi
fi

if [[ -z "$NEW_DATABASE_URL" ]]; then
    if [[ ! -f "$SIBLING_ENV" ]]; then
        echo "cannot derive DB credentials: ${SIBLING_ENV} is missing" >&2
        echo "pass --database-url postgres://oracle_app:PASSWORD@127.0.0.1:5432/${DB_NAME}" >&2
        exit 65
    fi
    sibling_url="$(grep -E '^DATABASE_URL=' "$SIBLING_ENV" | head -1 | cut -d= -f2-)"
    if ! parse_pg_url "$sibling_url"; then
        echo "could not parse DATABASE_URL in ${SIBLING_ENV}" >&2
        echo "pass --database-url postgres://oracle_app:PASSWORD@127.0.0.1:5432/${DB_NAME}" >&2
        exit 65
    fi
    NEW_DATABASE_URL="postgres://${PG_USER}:${PG_PASS}@${PG_HOST}:${PG_PORT}/${DB_NAME}"
    echo "[setup] reusing ${PG_USER}@${PG_HOST}:${PG_PORT} from onchain-transfer-devnet; new DB ${DB_NAME}"
fi

if ! parse_pg_url "$NEW_DATABASE_URL"; then
    echo "could not parse DATABASE_URL for postgres bring-up" >&2
    exit 65
fi

# ----- Postgres database (create if missing, migrate) -------------------------

db_exists="$(sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='${DB_NAME}'" || true)"
if [[ "$db_exists" != "1" ]]; then
    echo "[setup] creating database ${DB_NAME} owned by ${PG_USER}"
    sudo -u postgres createdb -O "$PG_USER" "$DB_NAME"
else
    echo "[setup] database ${DB_NAME} already exists"
fi

echo "[setup] applying ${INIT_SQL}"
PGPASSWORD="$PG_PASS" psql -U "$PG_USER" -h "$PG_HOST" -p "$PG_PORT" -d "$DB_NAME" -v ON_ERROR_STOP=1 -f "$INIT_SQL" >/dev/null

# ----- Env file ---------------------------------------------------------------

GENERATED_TOKEN=""
if [[ ! -f "$ENV_FILE" ]]; then
    install -m 0640 -o root -g root "$ENV_SRC" "$ENV_FILE"
    GENERATED_TOKEN="$(openssl rand -hex 32)"
    TOKEN_SHA="$(printf '%s' "$GENERATED_TOKEN" | sha256sum | awk '{print $1}')"
    echo "[setup] installed ${ENV_FILE} from example"
else
    echo "[setup] keeping existing ${ENV_FILE}"
    TOKEN_SHA=""
fi

tmp_env="$(mktemp)"
trap 'rm -f "$tmp_env"' EXIT
export NEW_DATABASE_URL TOKEN_SHA
awk '
    BEGIN { url=ENVIRON["NEW_DATABASE_URL"]; sha=ENVIRON["TOKEN_SHA"] }
    /^DATABASE_URL=/ { print "DATABASE_URL=" url; next }
    /^ORACLE_OPERATOR_TOKEN_SHA256=/ && sha != "" { print "ORACLE_OPERATOR_TOKEN_SHA256=" sha; next }
    { print }
' "$ENV_FILE" > "$tmp_env"
install -m 0640 -o root -g root "$tmp_env" "$ENV_FILE"

if [[ -n "$GENERATED_TOKEN" ]]; then
    umask 077
    printf '%s\n' "$GENERATED_TOKEN" > "$TOKEN_NOTE"
    chmod 0600 "$TOKEN_NOTE"
    echo "[setup] operator token (save once; SHA is in ${ENV_FILE}):"
    echo "        ${GENERATED_TOKEN}"
    echo "        also written to ${TOKEN_NOTE}"
fi

# Sanity: reserved ports must not appear as BIND_ADDR.
bind="$(grep -E '^BIND_ADDR=' "$ENV_FILE" | head -1 | cut -d= -f2-)"
if [[ "$bind" =~ :(4021|4031)$ ]]; then
    echo "BIND_ADDR=${bind} collides with onchain-transfer; expected :${BIND_PORT}" >&2
    exit 65
fi

# ----- systemd unit -----------------------------------------------------------

install -m 0644 -o root -g root "$UNIT_SRC" "/etc/systemd/system/${UNIT}.service"
systemctl daemon-reload
echo "[setup] installed /etc/systemd/system/${UNIT}.service"
systemctl enable "${UNIT}.service"

# ----- Deploy -----------------------------------------------------------------

if (( SKIP_DEPLOY )); then
    echo "[setup] --skip-deploy: host prep done. Next:"
    echo "        sudo bash ${SCRIPT_DIR}/oracle-file-delivery-devnet-deploy.sh"
    exit 0
fi

deploy_args=(--unit "$UNIT")
if (( SKIP_BUILD )); then
    deploy_args+=(--skip-build)
fi
echo "[setup] handing off to oracle-deploy.sh ${deploy_args[*]}"
bash "${SCRIPT_DIR}/oracle-deploy.sh" "${deploy_args[@]}"
