#!/usr/bin/env bash
# Vérifie la signature d'un `Distant.app` construit et affiche l'identité que
# macOS lui reconnaît (son « designated requirement ») : c'est elle que macOS
# enregistre avec les autorisations « Enregistrement de l'écran » et
# « Accessibilité ».
#
#   bash .github/scripts/verifier-signature-macos.sh chemin/vers/Distant.app
#
# Quand `APPLE_SIGNING_IDENTITY` désigne un certificat (posé par
# trousseau-macos.sh), échoue si l'application n'est pas signée avec lui.
set -euo pipefail

app="$1"
executable="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")"

codesign --verify --deep --strict "$app"

# Ce que lit Distant lui-même (`stable_signature` dans permissions.rs).
description="$(codesign -dv --verbose=2 "$executable" 2>&1)"
grep -E '^(Identifier|Format|CodeDirectory|Signature|Authority|TeamIdentifier)' <<<"$description" || true

exigence="$(codesign -d -r- "$app" 2>/dev/null | sed -n 's/^.*designated => //p')"
echo "Identité pour macOS : $exigence"

identite="${APPLE_SIGNING_IDENTITY:-}"
if [ -n "$identite" ] && [ "$identite" != "-" ]; then
  attendu="certificate leaf = H\"$(printf '%s' "$identite" | tr '[:upper:]' '[:lower:]')\""
  case "$exigence" in
    *"$attendu"*) ;;
    *)
      echo "::error title=Signature macOS::L'application n'est pas signée avec le certificat attendu ($identite)."
      exit 1
      ;;
  esac
  # La même règle que Distant : une autorité, et pas de signature ad hoc.
  if ! grep -q '^Authority=' <<<"$description" || grep -q '^Signature=adhoc' <<<"$description"; then
    echo "::error title=Signature macOS::Distant prendrait cette signature pour une signature ad hoc."
    exit 1
  fi
  resume="certificat fixe, les autorisations sont gardées d'une version à l'autre"
else
  resume="ad hoc, les autorisations seront à redonner après cette version"
fi

echo "Signature : $resume"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "**Signature macOS** de \`$(basename "$app")\` (${CIBLE:-$(uname -m)}) : $resume."
    echo
    echo "\`$exigence\`"
    echo
  } >> "$GITHUB_STEP_SUMMARY"
fi
