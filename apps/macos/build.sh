#!/bin/zsh
set -euo pipefail
cd "$(dirname "$0")"

CONFIG=${1:-release}
APP=build/Ciniva.app

swift build -c "$CONFIG" --product Ciniva
BIN=$(swift build -c "$CONFIG" --show-bin-path)/Ciniva

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Ciniva"
cp Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Ciniva</string>
    <key>CFBundleDisplayName</key><string>Ciniva</string>
    <key>CFBundleIdentifier</key><string>org.ciniva.mac</string>
    <key>CFBundleExecutable</key><string>Ciniva</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.entertainment</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSLocalNetworkUsageDescription</key><string>Ciniva connects to your Ciniva server on the local network.</string>
    <key>NSAppTransportSecurity</key>
    <dict><key>NSAllowsArbitraryLoads</key><true/></dict>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP" >/dev/null
echo "Built $APP"
