#!/bin/bash
set -euo pipefail

if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    printf '%s\n' 'Build Holod on a Mac with Apple Silicon.' >&2
    exit 1
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
root=$PWD
output="$root/dist"
target="$root/target/macos-package"
info_plist="$root/packaging/macos/Info.plist"
mkdir -p "$output"

metadata=$(cargo metadata --locked --no-deps --format-version 1)
version=$(printf '%s' "$metadata" | plutil -extract packages.0.version raw -o - -)
app_name=$(plutil -extract CFBundleName raw -o - "$info_plist")
if [[ -n ${CI_COMMIT_TAG:-} && "$CI_COMMIT_TAG" != "v$version" ]]; then
    printf 'The release tag must be v%s.\n' "$version" >&2
    exit 1
fi

sign_args=(--force --sign "${HOLOD_SIGN_IDENTITY:--}")
if [[ -n ${HOLOD_SIGN_IDENTITY:-} ]]; then
    sign_args+=(--timestamp)
    if [[ -n ${HOLOD_SIGN_KEYCHAIN:-} ]]; then
        sign_args+=(--keychain "$HOLOD_SIGN_KEYCHAIN")
    fi
fi
executable_sign_args=("${sign_args[@]}")
if [[ -n ${HOLOD_SIGN_IDENTITY:-} ]]; then
    executable_sign_args+=(--options runtime)
fi

if [[ -n ${HOLOD_SPARKLE_DIR:-} || -n ${HOLOD_APPCAST_URL:-} || -n ${HOLOD_SPARKLE_PUBLIC_KEY:-} ]]; then
    : "${HOLOD_SPARKLE_DIR:?Set the path to the extracted Sparkle distribution}"
    : "${HOLOD_APPCAST_URL:?Set the Sparkle appcast URL}"
    : "${HOLOD_SPARKLE_PUBLIC_KEY:?Set the Sparkle public key}"
    if [[ ! -f "$HOLOD_SPARKLE_DIR/Sparkle.framework/Sparkle" || ! -f "$HOLOD_SPARKLE_DIR/LICENSE" \
        || "$HOLOD_APPCAST_URL" != https://* \
        || ! "$HOLOD_SPARKLE_PUBLIC_KEY" =~ ^[A-Za-z0-9+/]{43}=$ ]]; then
        printf '%s\n' 'Sparkle requires its framework, an HTTPS appcast, and a valid EdDSA public key.' >&2
        exit 1
    fi
fi
cargo build --locked --release --target aarch64-apple-darwin --target-dir "$target" --bin holod

stage=$(mktemp -d "$output/.holod.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

volume="$stage/volume"
app="$volume/$app_name.app"
iconset="$stage/holod.iconset"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$iconset"
install -m 0755 "$target/aarch64-apple-darwin/release/holod" "$app/Contents/MacOS/holod"
install -m 0644 "$info_plist" "$app/Contents/Info.plist"

app_plist="$app/Contents/Info.plist"
if [[ -n ${HOLOD_SPARKLE_DIR:-} ]]; then
    framework="$app/Contents/Frameworks/Sparkle.framework"
    mkdir -p "$app/Contents/Frameworks"
    ditto "$HOLOD_SPARKLE_DIR/Sparkle.framework" "$framework"
    install -m 0644 "$HOLOD_SPARKLE_DIR/LICENSE" "$app/Contents/Resources/Sparkle-LICENSE.txt"
    # Holod runs outside App Sandbox and uses Sparkle without its optional XPC services.
    rm -rf -- "$framework/Versions/Current/XPCServices" "$framework/XPCServices"
    codesign "${executable_sign_args[@]}" "$framework/Autoupdate"
    codesign "${executable_sign_args[@]}" "$framework/Updater.app"
    codesign "${executable_sign_args[@]}" "$framework"

    plutil -insert SUFeedURL -string "$HOLOD_APPCAST_URL" "$app_plist"
    plutil -insert SUPublicEDKey -string "$HOLOD_SPARKLE_PUBLIC_KEY" "$app_plist"
    plutil -insert SUVerifyUpdateBeforeExtraction -bool YES "$app_plist"
    plutil -insert SURequireSignedFeed -bool YES "$app_plist"
    plutil -insert SUEnableAutomaticChecks -bool YES "$app_plist"
    plutil -insert SUAutomaticallyUpdate -bool NO "$app_plist"
fi

logo="$root/assets/holod-logo.png"
for size in 16 32 128 256 512; do
    icon="icon_${size}x${size}"
    retina=$((size * 2))
    sips -z "$size" "$size" "$logo" --out "$iconset/$icon.png" >/dev/null
    sips -z "$retina" "$retina" "$logo" --out "$iconset/$icon@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/holod.icns"

plutil -insert CFBundleShortVersionString -string "$version" "$app_plist"
plutil -insert CFBundleVersion -string "$version" "$app_plist"
plutil -lint "$app_plist"
codesign "${executable_sign_args[@]}" "$app"
codesign --verify --deep --strict "$app"

ln -s /Applications "$volume/Applications"
image_name="holod-${version}-macos-arm64.dmg"
hdiutil create -quiet -srcfolder "$volume" -volname "$app_name" -fs HFS+ -format UDZO "$stage/$image_name"
if [[ -n ${HOLOD_SIGN_IDENTITY:-} ]]; then
    codesign "${sign_args[@]}" "$stage/$image_name"
    codesign --verify --strict "$stage/$image_name"
fi
hdiutil verify -quiet "$stage/$image_name"

app_output="$output/$app_name.app"
previous_app="$stage/previous.app"
if [[ -e "$app_output" || -L "$app_output" ]]; then
    mv "$app_output" "$previous_app"
fi
if ! mv "$app" "$app_output"; then
    if [[ -e "$previous_app" || -L "$previous_app" ]]; then
        mv "$previous_app" "$app_output"
    fi
    exit 1
fi
mv -f "$stage/$image_name" "$output/$image_name"
printf 'Application: %s\nDisk image: %s\n' "$app_output" "$output/$image_name"
