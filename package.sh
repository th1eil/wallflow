#!/bin/bash
set -e

echo "Building WallFlow in release mode..."
cargo build --release

APP_DIR="target/release/WallFlow.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

echo "Creating app bundle structure..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR"
mkdir -p "$RESOURCES_DIR"

echo "Copying executable..."
cp target/release/wallflow "$MACOS_DIR/wallflow"

# --- Generate .icns from icon.png ---
# Note: the current source icon is 18x18. For production, use a >= 1024x1024
# source icon so the .icns is sharp at all Finder sizes.
echo "Generating icon.icns..."
ICONSET_DIR="$RESOURCES_DIR/icon.iconset"
mkdir -p "$ICONSET_DIR"

# Scale up the 18x18 source to required sizes.
# This produces blurry results at larger sizes — a larger source icon is recommended.
sips -z 16 16   assets/icon.png --out "$ICONSET_DIR/icon_16x16.png"
sips -z 32 32   assets/icon.png --out "$ICONSET_DIR/icon_16x16@2x.png"
sips -z 32 32   assets/icon.png --out "$ICONSET_DIR/icon_32x32.png"
sips -z 64 64   assets/icon.png --out "$ICONSET_DIR/icon_32x32@2x.png"
sips -z 128 128 assets/icon.png --out "$ICONSET_DIR/icon_128x128.png"
sips -z 256 256 assets/icon.png --out "$ICONSET_DIR/icon_256x256.png"
sips -z 256 256 assets/icon.png --out "$ICONSET_DIR/icon_128x128@2x.png"
sips -z 512 512 assets/icon.png --out "$ICONSET_DIR/icon_512x512.png"
sips -z 512 512 assets/icon.png --out "$ICONSET_DIR/icon_256x256@2x.png"
sips -z 1024 1024 assets/icon.png --out "$ICONSET_DIR/icon_512x512@2x.png"

iconutil -c icns "$ICONSET_DIR" -o "$RESOURCES_DIR/icon.icns"
rm -rf "$ICONSET_DIR"
echo "icon.icns created."

echo "Creating Info.plist..."
cat <<EOF > "$CONTENTS_DIR/Info.plist"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>wallflow</string>
    <key>CFBundleIdentifier</key>
    <string>com.rei.wallflow</string>
    <key>CFBundleName</key>
    <string>WallFlow</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# --- Create DMG ---
echo "Creating DMG..."
DMG_PATH="target/release/WallFlow.dmg"
rm -f "$DMG_PATH"

hdiutil create -fs HFS+ -volname "WallFlow" -srcfolder "$APP_DIR" -format UDZO "$DMG_PATH"

echo ""
echo "=========================================="
echo "WallFlow packaged successfully!"
echo "  App bundle: $APP_DIR"
echo "  DMG:        $DMG_PATH"
echo "=========================================="
echo "You can run: open '$APP_DIR'"
