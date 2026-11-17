#!/usr/bin/env bash
# Crée le certificat auto-signé qui signe les versions macOS de Distant.
#
# À lancer UNE SEULE FOIS, sur n'importe quel poste qui a OpenSSL (Git Bash
# sous Windows convient). macOS reconnaît l'application à l'empreinte de ce
# certificat : tant que toutes les versions sont signées avec lui, les
# autorisations données sur un Mac sont gardées d'une version à l'autre. En
# créer un autre, même au même nom, change l'empreinte, et tous les Mac
# redemandent leurs autorisations.
#
#   bash .github/scripts/certificat-macos.sh [dossier] [nom]
#
# Écrit dans `dossier` (par défaut ~/.tauri/distant-macos) :
#   distant-macos.p12              le certificat et sa clé privée
#   mot-de-passe.txt               le mot de passe du .p12
#   secret-MACOS_CERTIFICATE.txt   le .p12 en base64, sur une ligne
#   distant-macos.crt              le certificat seul (public)
#
# L'essai de signature (.github/workflows/essai-signature.yml) lance ce même
# script pour fabriquer un certificat jetable.
set -euo pipefail

dossier="${1:-$HOME/.tauri/distant-macos}"
nom="${2:-Distant}"
p12="$dossier/distant-macos.p12"

if [ -e "$p12" ]; then
  echo "Il y a déjà un certificat dans $dossier : il n'est pas remplacé." >&2
  echo "Un nouveau certificat ferait redemander leurs autorisations à tous les Mac." >&2
  exit 1
fi

umask 077
mkdir -p "$dossier"
travail="$(mktemp -d)"
trap 'rm -rf "$travail"' EXIT

# Le nom passe par un fichier et non par `-subj` : Git Bash prendrait
# « /CN=… » pour un chemin et le réécrirait.
cat > "$travail/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $nom
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

# Valable 20 ans : passé cette date, il faudra un nouveau certificat.
openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$travail/cert.cnf" \
  -keyout "$travail/cle.pem" -out "$dossier/distant-macos.crt" 2>/dev/null

# 3DES et SHA-1 : le seul chiffrement de .p12 que l'outil `security` de tous
# les macOS sait lire. Celui que choisit OpenSSL 3 de lui-même y est refusé.
P12_MOT_DE_PASSE="$(openssl rand -hex 24)"
export P12_MOT_DE_PASSE
openssl pkcs12 -export -inkey "$travail/cle.pem" -in "$dossier/distant-macos.crt" -name "$nom" \
  -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 \
  -passout env:P12_MOT_DE_PASSE -out "$p12"
printf '%s' "$P12_MOT_DE_PASSE" > "$dossier/mot-de-passe.txt"
openssl base64 -A -in "$p12" -out "$dossier/secret-MACOS_CERTIFICATE.txt"

empreinte="$(openssl x509 -in "$dossier/distant-macos.crt" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')"
echo "Certificat « $nom » créé dans $dossier"
echo "Empreinte SHA-1 : $empreinte"
echo "Expire le : $(openssl x509 -in "$dossier/distant-macos.crt" -noout -enddate | sed 's/.*=//')"
