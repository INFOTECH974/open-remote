#!/usr/bin/env bash
# Sur la machine de build macOS : importe le certificat de signature dans un
# trousseau temporaire et le désigne à Tauri.
#
# Lit deux variables, tirées des secrets du dépôt :
#   MACOS_CERTIFICATE            le .p12 en base64
#   MACOS_CERTIFICATE_PASSWORD   son mot de passe
#
# Sans certificat, ne fait rien : `tauri.conf.json` demande une signature
# « ad hoc » (`signingIdentity: "-"`) et le build continue ainsi.
#
# Le certificat est auto-signé : Tauri ne saurait pas l'importer lui-même
# (`APPLE_CERTIFICATE` ne retient que les certificats délivrés par Apple).
# Il signe en revanche avec l'identité que lui donne `APPLE_SIGNING_IDENTITY`,
# cherchée dans les trousseaux de l'utilisateur.
set -euo pipefail

if [ -z "${MACOS_CERTIFICATE:-}" ]; then
  echo "::warning title=Signature macOS::Secret MACOS_CERTIFICATE absent : signature ad hoc. Les Mac redemanderont leurs autorisations après cette version."
  exit 0
fi
if [ -z "${MACOS_CERTIFICATE_PASSWORD:-}" ]; then
  echo "::error title=Signature macOS::Secret MACOS_CERTIFICATE_PASSWORD absent alors que MACOS_CERTIFICATE est défini."
  exit 1
fi

temp="${RUNNER_TEMP:-$(mktemp -d)}"
trousseau="$temp/distant-signature.keychain-db"
p12="$temp/distant-macos.p12"
# Ce mot de passe ne protège que le trousseau, qui disparaît avec la machine.
cle_trousseau="$(openssl rand -hex 16)"

# Un secret collé avec des retours à la ligne reste lisible.
printf '%s' "$MACOS_CERTIFICATE" | tr -d '[:space:]' | openssl base64 -d -A -out "$p12"

security create-keychain -p "$cle_trousseau" "$trousseau"
# Verrouillage automatique repoussé à 2 h : bien plus qu'un build.
security set-keychain-settings -lut 7200 "$trousseau"
security unlock-keychain -p "$cle_trousseau" "$trousseau"
security import "$p12" -f pkcs12 -k "$trousseau" -P "$MACOS_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
rm -f "$p12"
# Sans cela, `codesign` attendrait une confirmation à l'écran pour lire la clé.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$cle_trousseau" "$trousseau" >/dev/null
# `codesign` cherche l'identité dans les trousseaux de l'utilisateur.
# shellcheck disable=SC2046
security list-keychains -d user -s "$trousseau" $(security list-keychains -d user | tr -d '"')

# Un certificat auto-signé n'est pas « de confiance » : il apparaît parmi les
# identités, pas parmi les identités valides. `codesign` s'en sert quand même.
identites="$(security find-identity -p codesigning "$trousseau" | sed -n '/Matching identities/,/identities found/p' | grep -E '^ +[0-9]+\) ' || true)"
if [ "$(printf '%s\n' "$identites" | grep -c .)" != 1 ]; then
  echo "::error title=Signature macOS::Le .p12 doit contenir une identité de signature de code, et une seule."
  printf '%s\n' "$identites"
  exit 1
fi
# L'empreinte SHA-1 désigne le certificat sans équivoque, mieux que son nom.
empreinte="$(printf '%s\n' "$identites" | awk '{print $2}')"
echo "Identité de signature :$(printf '%s\n' "$identites" | sed 's/^ *[0-9]*)//')"

if [ -n "${GITHUB_ENV:-}" ]; then
  echo "APPLE_SIGNING_IDENTITY=$empreinte" >> "$GITHUB_ENV"
  echo "DISTANT_TROUSSEAU=$trousseau" >> "$GITHUB_ENV"
fi
