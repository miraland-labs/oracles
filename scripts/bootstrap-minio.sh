#!/usr/bin/env bash
# Stand up a self-hosted MinIO server suitable for oracle-file-delivery and any
# oracle-common-based registry that needs S3-compatible blob storage.
#
# Usage:
#   sudo MINIO_ROOT_USER=... MINIO_ROOT_PASSWORD=... ./bootstrap-minio.sh
#
# Optional env (with defaults):
#   MINIO_BUCKET=oracle-blobs
#   MINIO_ADDR=127.0.0.1:9000
#   MINIO_CONSOLE_ADDR=127.0.0.1:9001
#   MINIO_DATA_DIR=/srv/minio
#
# Idempotent: re-running the script will not destroy data or recreate the bucket.

set -euo pipefail

: "${MINIO_ROOT_USER:?set MINIO_ROOT_USER}"
: "${MINIO_ROOT_PASSWORD:?set MINIO_ROOT_PASSWORD}"
MINIO_BUCKET="${MINIO_BUCKET:-oracle-blobs}"
MINIO_ADDR="${MINIO_ADDR:-127.0.0.1:9000}"
MINIO_CONSOLE_ADDR="${MINIO_CONSOLE_ADDR:-127.0.0.1:9001}"
MINIO_DATA_DIR="${MINIO_DATA_DIR:-/srv/minio}"

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)  MINIO_PKG=linux-amd64 ;;
  aarch64|arm64) MINIO_PKG=linux-arm64 ;;
  *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

# 1. install MinIO server + mc client
if ! command -v minio >/dev/null; then
  curl -fsSL -o /tmp/minio "https://dl.min.io/server/minio/release/${MINIO_PKG}/minio"
  install -m 0755 /tmp/minio /usr/local/bin/minio
fi
if ! command -v mc >/dev/null; then
  curl -fsSL -o /tmp/mc "https://dl.min.io/client/mc/release/${MINIO_PKG}/mc"
  install -m 0755 /tmp/mc /usr/local/bin/mc
fi

# 2. user / dirs
if ! id -u minio >/dev/null 2>&1; then
  useradd --system --home "${MINIO_DATA_DIR}" --shell /usr/sbin/nologin minio
fi
install -d -o minio -g minio "${MINIO_DATA_DIR}"

# 3. environment file (0600)
tee /etc/minio.env >/dev/null <<EOF
MINIO_ROOT_USER=${MINIO_ROOT_USER}
MINIO_ROOT_PASSWORD=${MINIO_ROOT_PASSWORD}
MINIO_VOLUMES=${MINIO_DATA_DIR}
MINIO_OPTS="--address ${MINIO_ADDR} --console-address ${MINIO_CONSOLE_ADDR}"
EOF
chmod 600 /etc/minio.env
chown root:root /etc/minio.env

# 4. systemd unit
tee /etc/systemd/system/minio.service >/dev/null <<'EOF'
[Unit]
Description=MinIO object storage (oracle blob backend)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=minio
Group=minio
EnvironmentFile=/etc/minio.env
ExecStart=/usr/local/bin/minio server $MINIO_VOLUMES $MINIO_OPTS
Restart=on-failure
RestartSec=5
LimitNOFILE=65535
ProtectSystem=full
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now minio.service

# 5. wait for liveness, then ensure bucket exists
for _ in $(seq 1 10); do
  if curl -fsS "http://${MINIO_ADDR}/minio/health/live" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

mc alias set oracle-local "http://${MINIO_ADDR}" "${MINIO_ROOT_USER}" "${MINIO_ROOT_PASSWORD}" >/dev/null
mc mb --ignore-existing "oracle-local/${MINIO_BUCKET}" >/dev/null
mc anonymous set none "oracle-local/${MINIO_BUCKET}" >/dev/null || true

cat <<EOF

MinIO ready.
  Endpoint: http://${MINIO_ADDR}
  Bucket:   ${MINIO_BUCKET}
  Console:  http://${MINIO_CONSOLE_ADDR}

Set in oracle .env:
  ORACLE_REGISTRY_BACKEND=s3
  ORACLE_REGISTRY_S3_ENDPOINT=http://${MINIO_ADDR}
  ORACLE_REGISTRY_S3_BUCKET=${MINIO_BUCKET}
  ORACLE_REGISTRY_S3_ACCESS_KEY=${MINIO_ROOT_USER}
  ORACLE_REGISTRY_S3_SECRET_KEY=<see /etc/minio.env>
  ORACLE_REGISTRY_S3_REGION=us-east-1
EOF
