#!/usr/bin/env bash
# oracle-nginx-setup.sh
# One-shot: install nginx (+optional certbot), write a reverse-proxy vhost for
# the two oracle-onchain-transfer instances, optionally issue Let's Encrypt
# certs, and flip the oracle env files back to loopback (Posture B).
#
# Four modes:
#
#  1) Domain mode — two hostnames (recommended for clean separation):
#       sudo ./oracle-nginx-setup.sh \
#         --devnet-host  oracle-devnet.example.com \
#         --mainnet-host oracle-mainnet.example.com \
#         --email you@example.com \
#         --flip-loopback
#     Requires real A records pointing to this host.
#
#  2) Single-host mode — one hostname, path-based, with TLS:
#       sudo ./oracle-nginx-setup.sh \
#         --single-host oracle.example.com \
#         --email you@example.com \
#         --flip-loopback
#     Requires one A record pointing to this host. Issues ONE Let's Encrypt
#     cert covering the single hostname. Routes:
#       https://oracle.example.com/devnet/...  -> 127.0.0.1:DEVNET_PORT
#       https://oracle.example.com/mainnet/... -> 127.0.0.1:MAINNET_PORT
#
#  3) Wildcard-DNS mode (no DNS needed, IP encoded in the hostname):
#       sudo ./oracle-nginx-setup.sh --nip --email you@example.com --flip-loopback
#     Defaults to nip.io. Use --sslip for sslip.io. Use --public-ip <ip> to
#     override IP detection when the host is behind NAT (Huawei Cloud, etc).
#     Issues TLS for oracle-devnet.<ip-with-dashes>.<wildcard> and
#                    oracle-mainnet.<ip-with-dashes>.<wildcard>.
#
#  4) IP-only mode (no DNS, no TLS, path-based routing on port 80):
#       sudo ./oracle-nginx-setup.sh --ip-only --flip-loopback
#     Exposes:
#       http://<ip>/devnet/...   -> 127.0.0.1:DEVNET_PORT
#       http://<ip>/mainnet/...  -> 127.0.0.1:MAINNET_PORT
#
# Common flags:
#   --devnet-port  4021   (default)
#   --mainnet-port 4031   (default)
#   --no-tls              (skip certbot in domain/single-host/sslip modes, plain http)
#   --flip-loopback       (rewrite /etc/oracle/*.env BIND_ADDR to 127.0.0.1
#                          and remove public ufw rules for those ports)
#
# Idempotent. Re-runnable.
set -euo pipefail

DEVNET_HOST=""
MAINNET_HOST=""
SINGLE_HOST=""
EMAIL=""
DEVNET_PORT=4021
MAINNET_PORT=4031
DO_TLS=1
FLIP_LOOPBACK=0
WILDCARD_MODE=""   # "" | "nip.io" | "sslip.io"
PUBLIC_IP=""
IP_ONLY=0

die() { echo "ERROR: $*" >&2; exit 1; }
log() { echo "[oracle-nginx] $*"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --devnet-host)   DEVNET_HOST="$2"; shift 2 ;;
    --mainnet-host)  MAINNET_HOST="$2"; shift 2 ;;
    --single-host)   SINGLE_HOST="$2"; shift 2 ;;
    --email)         EMAIL="$2"; shift 2 ;;
    --devnet-port)   DEVNET_PORT="$2"; shift 2 ;;
    --mainnet-port)  MAINNET_PORT="$2"; shift 2 ;;
    --no-tls)        DO_TLS=0; shift ;;
    --flip-loopback) FLIP_LOOPBACK=1; shift ;;
    --nip)           WILDCARD_MODE="nip.io"; shift ;;
    --sslip)         WILDCARD_MODE="sslip.io"; shift ;;
    --public-ip)     PUBLIC_IP="$2"; shift 2 ;;
    --ip-only)       IP_ONLY=1; DO_TLS=0; shift ;;
    -h|--help)
      sed -n '2,52p' "$0"; exit 0 ;;
    *) die "unknown arg: $1" ;;
  esac
done

[[ $EUID -eq 0 ]] || die "must run as root (use sudo)"

# detect public IP if needed (prefers --public-ip, else asks an external echo)
detect_ip() {
  if [[ -n "$PUBLIC_IP" ]]; then
    echo "$PUBLIC_IP"; return
  fi
  local ip=""
  for url in https://api.ipify.org https://ifconfig.me https://ipv4.icanhazip.com; do
    ip=$(curl -fsS --max-time 5 "$url" 2>/dev/null | tr -d '[:space:]') || true
    [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] && { echo "$ip"; return; }
  done
  # fallback: local route src (likely wrong on NAT'd hosts; warn)
  ip -4 route get 1.1.1.1 2>/dev/null \
    | awk '{for(i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}'
}

# Validate exclusive mode flags upfront — exactly one mode must be active.
MODES_SET=0
[[ -n "$SINGLE_HOST"   ]] && MODES_SET=$((MODES_SET+1))
[[ -n "$WILDCARD_MODE" ]] && MODES_SET=$((MODES_SET+1))
[[ $IP_ONLY -eq 1      ]] && MODES_SET=$((MODES_SET+1))
[[ -n "$DEVNET_HOST$MAINNET_HOST" && -z "$SINGLE_HOST" && -z "$WILDCARD_MODE" && $IP_ONLY -eq 0 ]] && MODES_SET=$((MODES_SET+1))
[[ $MODES_SET -le 1 ]] || die "modes are exclusive: pick one of --single-host / --devnet-host+--mainnet-host / --nip|--sslip / --ip-only"

if [[ -n "$WILDCARD_MODE" ]]; then
  IP=$(detect_ip)
  [[ -n "$IP" ]] || die "could not detect host public IP; pass --public-ip <ip>"
  if [[ "$IP" =~ ^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.) ]]; then
    die "detected RFC1918 IP $IP — host is NAT'd. Pass --public-ip <real-public-ip>."
  fi
  IP_DASH=${IP//./-}
  DEVNET_HOST="oracle-devnet.${IP_DASH}.${WILDCARD_MODE}"
  MAINNET_HOST="oracle-mainnet.${IP_DASH}.${WILDCARD_MODE}"
  log "${WILDCARD_MODE} mode: devnet=$DEVNET_HOST mainnet=$MAINNET_HOST"
fi

if [[ $IP_ONLY -eq 1 ]]; then
  :  # no host args expected
elif [[ -n "$SINGLE_HOST" ]]; then
  log "single-host mode: $SINGLE_HOST (paths /devnet/ and /mainnet/)"
else
  [[ -n "$DEVNET_HOST"  ]] || die "--devnet-host is required (or use --single-host / --nip / --sslip / --ip-only)"
  [[ -n "$MAINNET_HOST" ]] || die "--mainnet-host is required (or use --single-host / --nip / --sslip / --ip-only)"
fi

if [[ $DO_TLS -eq 1 && -z "$EMAIL" ]]; then
  die "--email is required for TLS issuance (or pass --no-tls / --ip-only)"
fi

# 1. install packages -------------------------------------------------------
log "installing nginx${DO_TLS:+ + certbot}"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq nginx
if [[ $DO_TLS -eq 1 ]]; then
  apt-get install -y -qq certbot python3-certbot-nginx
fi

# 2. open firewall ----------------------------------------------------------
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  log "opening 80/443 in ufw"
  ufw allow 80/tcp  >/dev/null
  ufw allow 443/tcp >/dev/null
fi

# 3. write vhost ------------------------------------------------------------
VHOST=/etc/nginx/sites-available/oracle-onchain-transfer
log "writing $VHOST"

if [[ $IP_ONLY -eq 1 ]]; then
  cat >"$VHOST" <<EOF
# Managed by oracle-nginx-setup.sh — IP-only, path-based reverse proxy
# /devnet/*  -> 127.0.0.1:$DEVNET_PORT
# /mainnet/* -> 127.0.0.1:$MAINNET_PORT

server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name _;

    client_max_body_size 32m;
    proxy_read_timeout 30s;
    proxy_send_timeout 30s;

    location /devnet/ {
        rewrite ^/devnet/(.*)\$ /\$1 break;
        proxy_pass         http://127.0.0.1:$DEVNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }

    location /mainnet/ {
        rewrite ^/mainnet/(.*)\$ /\$1 break;
        proxy_pass         http://127.0.0.1:$MAINNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }

    location = / {
        return 200 "oracle-onchain-transfer reverse proxy\nuse /devnet/... or /mainnet/...\n";
        default_type text/plain;
    }
}
EOF
elif [[ -n "$SINGLE_HOST" ]]; then
  cat >"$VHOST" <<EOF
# Managed by oracle-nginx-setup.sh — single-host, path-based reverse proxy
# Host: $SINGLE_HOST
#   /devnet/*  -> 127.0.0.1:$DEVNET_PORT
#   /mainnet/* -> 127.0.0.1:$MAINNET_PORT
# certbot --nginx will add the TLS server block + http→https redirect after
# the initial port-80 vhost passes the ACME http-01 challenge.

server {
    listen 80;
    listen [::]:80;
    server_name $SINGLE_HOST;

    client_max_body_size 32m;
    proxy_read_timeout 30s;
    proxy_send_timeout 30s;

    location /devnet/ {
        rewrite ^/devnet/(.*)\$ /\$1 break;
        proxy_pass         http://127.0.0.1:$DEVNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }

    location /mainnet/ {
        rewrite ^/mainnet/(.*)\$ /\$1 break;
        proxy_pass         http://127.0.0.1:$MAINNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }

    location = / {
        return 200 "oracle-onchain-transfer reverse proxy at $SINGLE_HOST\nuse /devnet/... or /mainnet/...\n";
        default_type text/plain;
    }
}
EOF
else
  cat >"$VHOST" <<EOF
# Managed by oracle-nginx-setup.sh — reverse proxy for oracle-onchain-transfer
# Devnet  : $DEVNET_HOST  -> 127.0.0.1:$DEVNET_PORT
# Mainnet : $MAINNET_HOST -> 127.0.0.1:$MAINNET_PORT

server {
    listen 80;
    listen [::]:80;
    server_name $DEVNET_HOST;

    client_max_body_size 32m;
    proxy_read_timeout 30s;
    proxy_send_timeout 30s;

    location / {
        proxy_pass         http://127.0.0.1:$DEVNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }
}

server {
    listen 80;
    listen [::]:80;
    server_name $MAINNET_HOST;

    client_max_body_size 32m;
    proxy_read_timeout 30s;
    proxy_send_timeout 30s;

    location / {
        proxy_pass         http://127.0.0.1:$MAINNET_PORT;
        proxy_http_version 1.1;
        proxy_set_header   Host              \$host;
        proxy_set_header   X-Real-IP         \$remote_addr;
        proxy_set_header   X-Forwarded-For   \$proxy_add_x_forwarded_for;
        proxy_set_header   X-Forwarded-Proto \$scheme;
    }
}
EOF
fi

ln -sf "$VHOST" /etc/nginx/sites-enabled/oracle-onchain-transfer
# remove the stock default vhost only if it's a symlink (we don't trample real configs)
[[ -L /etc/nginx/sites-enabled/default ]] && rm -f /etc/nginx/sites-enabled/default

log "nginx -t"
nginx -t
systemctl enable --now nginx >/dev/null
systemctl reload nginx
log "nginx reloaded"

# 4. issue TLS --------------------------------------------------------------
if [[ $DO_TLS -eq 1 ]]; then
  if [[ -n "$SINGLE_HOST" ]]; then
    log "running certbot for $SINGLE_HOST"
    certbot --nginx --non-interactive --agree-tos --redirect \
      --email "$EMAIL" \
      -d "$SINGLE_HOST"
  else
    log "running certbot for $DEVNET_HOST and $MAINNET_HOST"
    certbot --nginx --non-interactive --agree-tos --redirect \
      --email "$EMAIL" \
      -d "$DEVNET_HOST" -d "$MAINNET_HOST"
  fi
  systemctl reload nginx
fi

# 5. optionally flip oracle env to loopback (Posture B) ---------------------
flip() {
  local f="$1" port="$2"
  [[ -f "$f" ]] || { log "skip $f (not present)"; return; }
  if grep -qE '^BIND_ADDR=0\.0\.0\.0:' "$f"; then
    log "flipping $f -> 127.0.0.1:$port"
    sed -i.bak -E "s|^BIND_ADDR=0\\.0\\.0\\.0:[0-9]+|BIND_ADDR=127.0.0.1:$port|" "$f"
  else
    log "no 0.0.0.0 BIND_ADDR in $f, leaving as-is"
  fi
}

if [[ $FLIP_LOOPBACK -eq 1 ]]; then
  flip /etc/oracle/onchain-transfer-devnet.env  "$DEVNET_PORT"
  flip /etc/oracle/onchain-transfer-mainnet.env "$MAINNET_PORT"
  log "restarting oracle units"
  systemctl restart oracle-onchain-transfer-devnet  || true
  systemctl restart oracle-onchain-transfer-mainnet || true
  if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
    log "removing public ufw rules for $DEVNET_PORT/$MAINNET_PORT"
    ufw delete allow "$DEVNET_PORT/tcp"  >/dev/null 2>&1 || true
    ufw delete allow "$MAINNET_PORT/tcp" >/dev/null 2>&1 || true
  fi
fi

# 6. verify -----------------------------------------------------------------
SCHEME="http"; [[ $DO_TLS -eq 1 ]] && SCHEME="https"
log "verifying:"
if [[ $IP_ONLY -eq 1 ]]; then
  IP=$(detect_ip)
  for prefix in devnet mainnet; do
    url="http://$IP/$prefix/v1/registry/info"
    code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 "$url" || echo 000)
    echo "  $url -> HTTP $code"
  done
elif [[ -n "$SINGLE_HOST" ]]; then
  for prefix in devnet mainnet; do
    url="$SCHEME://$SINGLE_HOST/$prefix/v1/registry/info"
    code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 "$url" || echo 000)
    echo "  $url -> HTTP $code"
  done
else
  for h in "$DEVNET_HOST" "$MAINNET_HOST"; do
    url="$SCHEME://$h/v1/registry/info"
    code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 "$url" || echo 000)
    echo "  $url -> HTTP $code"
  done
fi

log "done"
