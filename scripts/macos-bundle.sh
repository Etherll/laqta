#!/bin/sh
# Builds Laqta.app from two built binaries and signs it.
#   scripts/macos-bundle.sh <laqta> <laqta-nmh> <version> <out-dir>
# With MACOS_SIGN_IDENTITY set (a "Developer ID Application: ..." name in the keychain) the app is
# signed for distribution with the hardened runtime; without it the signature is ad hoc, which
# runs locally and in CI but is not accepted by Gatekeeper on other Macs.
set -eu
laqta=$1 nmh=$2 version=$3 out=$4
root=$(cd "$(dirname "$0")/.." && pwd)
app="$out/Laqta.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$laqta" "$app/Contents/MacOS/laqta"
cp "$nmh" "$app/Contents/MacOS/laqta-nmh"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"

# iconutil wants each size under a fixed name.
set_dir="$out/laqta.iconset"
rm -rf "$set_dir" && mkdir -p "$set_dir"
for size in 16 32 128 256 512; do
  cp "$root/assets/icon/laqta-$size.png" "$set_dir/icon_${size}x${size}.png"
  cp "$root/assets/icon/laqta-$((size * 2)).png" "$set_dir/icon_${size}x${size}@2x.png" 2>/dev/null || true
done
cp "$root/assets/icon/laqta-64.png" "$set_dir/icon_32x32@2x.png"
iconutil -c icns "$set_dir" -o "$app/Contents/Resources/laqta.icns"
rm -rf "$set_dir"

identity=${MACOS_SIGN_IDENTITY:--}
entitlements="$root/packaging/macos/entitlements.plist"
if [ "$identity" = "-" ]; then
  codesign --force --sign - --entitlements "$entitlements" "$app/Contents/MacOS/laqta-nmh"
  codesign --force --sign - --entitlements "$entitlements" "$app"
else
  codesign --force --options runtime --timestamp --sign "$identity" "$app/Contents/MacOS/laqta-nmh"
  codesign --force --options runtime --timestamp --sign "$identity" --entitlements "$entitlements" "$app"
fi
codesign --verify --deep --strict "$app"
echo "$app"
