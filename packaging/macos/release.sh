#!/bin/bash
set +x
set -euo pipefail
umask 077

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
: "${HOLOD_RELEASE_TAG:?Set the vX.Y.Z release tag}"
: "${HOLOD_SIGN_IDENTITY:?Set the Developer ID Application signing identity}"
: "${HOLOD_SIGN_P12_BASE64:?Set the Base64-encoded PKCS#12}"
: "${HOLOD_SIGN_P12_PASSWORD:?Set the PKCS#12 export password}"
: "${HOLOD_NOTARY_KEY:?Set the Apple API .p8 key}"
: "${HOLOD_NOTARY_KEY_ID:?Set the Apple API key ID}"
: "${HOLOD_NOTARY_ISSUER_ID:?Set the Apple API issuer ID}"

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/holod-release.XXXXXX")
keychain="$work_dir/signing.keychain-db"
cleanup() {
    if [[ -e "$keychain" ]]; then
        security delete-keychain "$keychain" || true
    fi
    rm -rf -- "$work_dir"
}
trap cleanup EXIT

p12="$work_dir/signing.p12"
notary_key="$work_dir/notary.p8"
printf '%s' "$HOLOD_SIGN_P12_BASE64" | base64 -D > "$p12"
printf '%s\n' "$HOLOD_NOTARY_KEY" > "$notary_key"
keychain_password=$(openssl rand -hex 24)
security create-keychain -p "$keychain_password" "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security import "$p12" -k "$keychain" -P "$HOLOD_SIGN_P12_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
rm "$p12"
unset keychain_password HOLOD_SIGN_P12_BASE64 HOLOD_SIGN_P12_PASSWORD HOLOD_NOTARY_KEY

(umask 022; HOLOD_SIGN_KEYCHAIN="$keychain" ./packaging/macos/build.sh)
image="dist/holod-${HOLOD_RELEASE_TAG#v}-macos-arm64.dmg"
xcrun notarytool submit "$image" --key "$notary_key" --key-id "$HOLOD_NOTARY_KEY_ID" \
    --issuer "$HOLOD_NOTARY_ISSUER_ID" --wait --timeout 40m
xcrun stapler staple "$image"
xcrun stapler validate "$image"
codesign --verify --strict "$image"
hdiutil verify -quiet "$image"
