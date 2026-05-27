#!/usr/bin/env bash
# tls/generate.sh — generate a self-signed TLS certificate for agent2web
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")"; pwd)"

openssl req \
  -x509 \
  -newkey rsa:4096 \
  -keyout "${SCRIPT_DIR}/key.pem" \
  -out    "${SCRIPT_DIR}/cert.pem" \
  -sha256 \
  -days   3650 \
  -nodes \
  -subj   "/CN=agent2web" \
  -addext "subjectAltName=IP:127.0.0.1,IP:::1,DNS:localhost"

echo "Done."
echo "  Certificate: ${SCRIPT_DIR}/cert.pem"
echo "  Private key: ${SCRIPT_DIR}/key.pem"
echo
echo "Add the following to agent2web.toml:"
echo "  [server.tls]"
echo "  enabled = true"
echo "  cert    = \"tls/cert.pem\""
echo "  key     = \"tls/key.pem\""
