#!/bin/bash
set +x
set -euo pipefail
umask 077

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
: "${CI_COMMIT_TAG:?Create a version tag to release Holod}"
: "${HOLOD_SIGN_IDENTITY:?Set the Developer ID Application signing identity}"
: "${HOLOD_SIGN_P12_BASE64:?Provide the Base64 PKCS#12 as a GitLab file variable}"
: "${HOLOD_SIGN_P12_PASSWORD:?Set the PKCS#12 export password}"
: "${HOLOD_NOTARY_KEY:?Provide the Apple API .p8 key as a GitLab file variable}"
: "${HOLOD_NOTARY_KEY_ID:?Set the Apple API key ID}"
: "${HOLOD_NOTARY_ISSUER_ID:?Set the Apple API issuer ID}"
: "${HOLOD_SPARKLE_PRIVATE_KEY:?Provide the exported Sparkle key as a GitLab file variable}"

version=${CI_COMMIT_TAG#v}
pages_url=${HOLOD_PAGES_URL:-${CI_PAGES_URL:-}}
pages_url=${pages_url%/}
if [[ ! "$CI_COMMIT_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ || "$pages_url" != https://* ]]; then
    printf '%s\n' 'A vX.Y.Z tag and a public HTTPS Pages URL are required.' >&2
    exit 1
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/holod-release.XXXXXX")
keychain="$work_dir/signing.keychain-db"
cleanup() {
    if [[ -e "$keychain" ]]; then
        security delete-keychain "$keychain" || true
    fi
    rm -rf -- "$work_dir"
}
trap cleanup EXIT

sparkle_version=2.10.0
sparkle_sha256=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c
sparkle_archive="$work_dir/Sparkle.tar.xz"
curl --fail --silent --show-error --location --retry 3 --max-time 120 \
    "https://github.com/sparkle-project/Sparkle/releases/download/$sparkle_version/Sparkle-$sparkle_version.tar.xz" \
    --output "$sparkle_archive"
printf '%s  %s\n' "$sparkle_sha256" "$sparkle_archive" | shasum -a 256 --check
tar -xf "$sparkle_archive" -C "$work_dir"

export HOLOD_SPARKLE_DIR="$work_dir"
export HOLOD_APPCAST_URL="$pages_url/appcast.xml"
export HOLOD_SIGN_KEYCHAIN="$keychain"
# Use the same Ed25519 seed for the public key in Holod and the feed signatures.
HOLOD_SPARKLE_PUBLIC_KEY=$(xcrun swift -e '
    import CryptoKit
    import Foundation
    enum KeyError: Error { case invalidSparkleSeed }
    guard let encoded = readLine(), let seed = Data(base64Encoded: encoded), seed.count == 32 else {
        throw KeyError.invalidSparkleSeed
    }
    let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    print(key.publicKey.rawRepresentation.base64EncodedString())
' < "$HOLOD_SPARKLE_PRIVATE_KEY")
export HOLOD_SPARKLE_PUBLIC_KEY

current_feed="$work_dir/current.xml"
status=$(curl --silent --show-error --location --retry 3 --max-time 60 \
    --write-out '%{http_code}' --output "$current_feed" "$HOLOD_APPCAST_URL")
case "$status" in
    200)
        "$work_dir/bin/sign_update" --verify --ed-key-file "$HOLOD_SPARKLE_PRIVATE_KEY" "$current_feed"
        python3 - "$current_feed" "$version" <<'PY'
import sys
import xml.etree.ElementTree as ET

feed, proposed = sys.argv[1:]
namespace = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
items = ET.parse(feed).findall("./channel/item")
versions = [item.findtext(f"{namespace}version") for item in items]
if not versions or not all(versions):
    sys.exit("The published appcast has no version to compare.")

def version_tuple(value):
    return tuple(map(int, value.split(".")))

latest = max(versions, key=version_tuple)
if version_tuple(proposed) <= version_tuple(latest):
    sys.exit(f"Version {proposed} must be newer than the published version {latest}.")
PY
        ;;
    404) ;;
    *) printf 'The current appcast returned HTTP %s.\n' "$status" >&2; exit 1 ;;
esac

p12="$work_dir/signing.p12"
keychain_password=$(openssl rand -hex 24)
base64 -D -i "$HOLOD_SIGN_P12_BASE64" -o "$p12"
security create-keychain -p "$keychain_password" "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security import "$p12" -k "$keychain" -P "$HOLOD_SIGN_P12_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
rm "$p12"
unset keychain_password HOLOD_SIGN_P12_PASSWORD

(umask 022; ./packaging/macos/build.sh)
image_name="holod-$version-macos-arm64.dmg"
image="dist/$image_name"
notary_auth=(--key "$HOLOD_NOTARY_KEY" --key-id "$HOLOD_NOTARY_KEY_ID" --issuer "$HOLOD_NOTARY_ISSUER_ID")
xcrun notarytool submit "$image" "${notary_auth[@]}" --wait --timeout 40m
xcrun stapler staple "$image"
xcrun stapler validate "$image"
codesign --verify --strict "$image"
hdiutil verify -quiet "$image"

mkdir public
install -m 0644 "$image" "public/$image_name"
if [[ -n ${CI_RELEASE_DESCRIPTION:-} ]]; then
    printf '%s\n' "$CI_RELEASE_DESCRIPTION" > "public/${image_name%.dmg}.md"
fi
"$work_dir/bin/generate_appcast" --ed-key-file "$HOLOD_SPARKLE_PRIVATE_KEY" \
    --download-url-prefix "$pages_url/" --link "$pages_url/" --embed-release-notes --maximum-deltas 0 \
    -o public/appcast.xml public
"$work_dir/bin/sign_update" --verify --ed-key-file "$HOLOD_SPARKLE_PRIVATE_KEY" public/appcast.xml
rm -f "public/${image_name%.dmg}.md"
printf '<!doctype html><meta charset="utf-8"><title>Holod</title><a href="%s">Download Holod %s for Apple Silicon</a>\n' \
    "$image_name" "$version" > public/index.html
chmod 0755 public
chmod 0644 public/appcast.xml public/index.html
