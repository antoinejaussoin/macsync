#!/usr/bin/env bash
# Builds dist/MacSync.app (menu-bar only, no Dock icon).
#   scripts/bundle.sh            build the bundle
#   scripts/bundle.sh --install  also copy it to /Applications
# Set SIGN_IDENTITY to a Developer ID / Apple Development certificate name to keep
# the Full Disk Access grant across rebuilds; the default ad-hoc signature changes
# with every build, so macOS asks again.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
VERSION="$(sed -n 's/.*static let version = "\(.*\)".*/\1/p' Sources/MacSync/MacSyncApp.swift | head -1)"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"

swift build -c release --product MacSync
BIN="$(swift build -c release --show-bin-path)/MacSync"
APP="$ROOT/dist/MacSync.app"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/MacSync"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>MacSync</string>
    <key>CFBundleDisplayName</key>
    <string>MacSync</string>
    <key>CFBundleIdentifier</key>
    <string>com.macsync.app</string>
    <key>CFBundleExecutable</key>
    <string>MacSync</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

codesign --force --sign "${SIGN_IDENTITY:--}" "$APP"
echo "Built $APP (version $VERSION)"

if [[ "${1:-}" == "--install" ]]; then
    if pgrep -x MacSync >/dev/null; then
        echo "Quit MacSync from its menu-bar icon first." >&2
        exit 1
    fi
    rm -rf /Applications/MacSync.app
    cp -R "$APP" /Applications/
    echo "Installed /Applications/MacSync.app"
fi
