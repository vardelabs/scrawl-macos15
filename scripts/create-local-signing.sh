#!/usr/bin/env bash
# Creates the self-signed "Scrawl Local Signing" code-signing identity in the login keychain, so
# install-app.sh signs every rebuild with the same identity and macOS keeps Scrawl's permission
# grants. Without it, install-app.sh signs ad hoc and the grants reset on each build.
set -euo pipefail

NAME="Scrawl Local Signing"
if security find-identity -p codesigning 2>/dev/null | grep -q "\"${NAME}\""; then
  echo "${NAME} already exists in the keychain."
  exit 0
fi

WORK="$(mktemp -d)"
chmod 700 "${WORK}"
trap 'rm -rf "${WORK}"' EXIT
cd "${WORK}"

cat > cfg <<'EOF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = Scrawl Local Signing
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF

/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 3650 \
  -config cfg 2>/dev/null
# The passphrase only carries the key into the keychain, so it is random and never shown.
PASS="$(/usr/bin/openssl rand -hex 16)"
/usr/bin/openssl pkcs12 -export -inkey key.pem -in cert.pem -out id.p12 -passout "pass:${PASS}" \
  2>/dev/null
security import id.p12 -k "${HOME}/Library/Keychains/login.keychain-db" -P "${PASS}" \
  -T /usr/bin/codesign >/dev/null
unset PASS

security find-identity -p codesigning | grep "\"${NAME}\""
