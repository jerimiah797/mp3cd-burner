#!/bin/bash
# Bundle MP3 CD Burner as a macOS .app

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
APP_NAME="MP3 CD Burner"
BUNDLE_NAME="${APP_NAME}.app"
BUILD_DIR="${PROJECT_DIR}/target/release"
BUNDLE_DIR="${BUILD_DIR}/${BUNDLE_NAME}"

# Code signing identity (from Apple Developer account)
SIGNING_IDENTITY="Developer ID Application: Jerimiah Ham (3QUH73KW5Q)"

# Parse arguments
SIGN=false
UNIVERSAL=false
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --sign) SIGN=true ;;
        --universal) UNIVERSAL=true ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
    shift
done

echo "=== Building MP3 CD Burner for macOS ==="

if [ "$UNIVERSAL" = true ]; then
    echo "Building universal binary (ARM64 + x86_64)..."

    # Build for ARM64
    echo "  Building for aarch64-apple-darwin..."
    cargo build --release --target aarch64-apple-darwin

    # Build for x86_64
    echo "  Building for x86_64-apple-darwin..."
    cargo build --release --target x86_64-apple-darwin

    # Create universal binary with lipo
    echo "  Creating universal binary with lipo..."
    mkdir -p "${BUILD_DIR}"
    lipo -create \
        "${PROJECT_DIR}/target/aarch64-apple-darwin/release/MP3-CD-Burner" \
        "${PROJECT_DIR}/target/x86_64-apple-darwin/release/MP3-CD-Burner" \
        -output "${BUILD_DIR}/MP3-CD-Burner"

    echo "  Universal binary created!"
else
    # Build release binary for current architecture only
    echo "Building release binary..."
    cargo build --release
fi

# Create app bundle structure
echo "Creating app bundle..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
mkdir -p "${BUNDLE_DIR}/Contents/Resources"

# Copy binary
echo "Copying binary..."
cp "${BUILD_DIR}/MP3-CD-Burner" "${BUNDLE_DIR}/Contents/MacOS/"

# Copy Info.plist
echo "Copying Info.plist..."
cp "${PROJECT_DIR}/macos/Info.plist" "${BUNDLE_DIR}/Contents/"

# Copy resources (ffmpeg binary)
echo "Copying resources..."
mkdir -p "${BUNDLE_DIR}/Contents/Resources/bin"
cp "${PROJECT_DIR}/resources/bin/ffmpeg" "${BUNDLE_DIR}/Contents/Resources/bin/"

# Copy icon if it exists
if [ -f "${PROJECT_DIR}/macos/AppIcon.icns" ]; then
    echo "Copying app icon..."
    cp "${PROJECT_DIR}/macos/AppIcon.icns" "${BUNDLE_DIR}/Contents/Resources/"
else
    echo "Note: No AppIcon.icns found - app will use default icon"
fi

# Copy PNG icon for About dialog
if [ -f "${PROJECT_DIR}/macos/icon_128.png" ]; then
    echo "Copying PNG icon..."
    cp "${PROJECT_DIR}/macos/icon_128.png" "${BUNDLE_DIR}/Contents/Resources/"
fi

# Create PkgInfo
echo -n "APPL????" > "${BUNDLE_DIR}/Contents/PkgInfo"

# Code signing
if [ "$SIGN" = true ]; then
    echo ""
    echo "=== Code Signing ==="

    # Sign the ffmpeg binary first (nested code must be signed first)
    echo "Signing ffmpeg..."
    codesign --force --options runtime --sign "${SIGNING_IDENTITY}" \
        "${BUNDLE_DIR}/Contents/Resources/bin/ffmpeg"

    # Sign the main binary
    echo "Signing main binary..."
    codesign --force --options runtime --sign "${SIGNING_IDENTITY}" \
        "${BUNDLE_DIR}/Contents/MacOS/MP3-CD-Burner"

    # Sign the entire app bundle
    echo "Signing app bundle..."
    codesign --force --options runtime --sign "${SIGNING_IDENTITY}" \
        "${BUNDLE_DIR}"

    # Verify signature
    echo "Verifying signature..."
    codesign --verify --deep --strict --verbose=2 "${BUNDLE_DIR}"

    echo "Code signing complete!"
else
    echo ""
    echo "Note: App is NOT signed. Use --sign to code sign for distribution."
fi

echo ""
echo "=== Build Complete ==="
echo "App bundle created at: ${BUNDLE_DIR}"
echo ""
echo "To run: open \"${BUNDLE_DIR}\""
echo ""
echo "To install: drag to /Applications"
