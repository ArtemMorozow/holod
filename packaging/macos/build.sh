#!/bin/bash
set -euo pipefail

if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    printf '%s\n' 'Build holod on a Mac with Apple Silicon.' >&2
    exit 1
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
root=$PWD
output="$root/dist"
target="$root/target/macos-package"
mkdir -p "$output"

metadata=$(cargo metadata --locked --no-deps --format-version 1)
version=$(printf '%s' "$metadata" | plutil -extract packages.0.version raw -o - -)
cargo build --locked --release --target aarch64-apple-darwin --target-dir "$target" --bin holod

stage=$(mktemp -d "$output/.holod.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT

volume="$stage/volume"
app="$volume/holod.app"
iconset="$stage/holod.iconset"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$iconset"
install -m 0755 "$target/aarch64-apple-darwin/release/holod" "$app/Contents/MacOS/holod"
install -m 0644 "$root/packaging/macos/Info.plist" "$app/Contents/Info.plist"

logo="$root/assets/holod-logo.png"
for size in 16 32 128 256 512; do
    icon="icon_${size}x${size}"
    retina=$((size * 2))
    sips -z "$size" "$size" "$logo" --out "$iconset/$icon.png" >/dev/null
    sips -z "$retina" "$retina" "$logo" --out "$iconset/$icon@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/holod.icns"

plutil -insert CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -insert CFBundleVersion -string "$version" "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
codesign --sign - --force "$app"
codesign --verify --strict "$app"

ln -s /Applications "$volume/Applications"
image_name="holod-${version}-macos-arm64.dmg"
hdiutil create -quiet -srcfolder "$volume" -volname holod -fs HFS+ -format UDZO "$stage/$image_name"
hdiutil verify -quiet "$stage/$image_name"

app_output="$output/holod.app"
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
