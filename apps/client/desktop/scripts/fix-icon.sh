#!/bin/bash
# Post-build script to fix icon placement for macOS 26+ (Liquid Glass .icon format)

APP_PATH="$1"

if [ -z "$APP_PATH" ]; then
    APP_PATH="target/release/bundle/macos/Cteno.app"
fi

if [ -d "$APP_PATH" ]; then
    RESOURCES="$APP_PATH/Contents/Resources"

    # Copy Cteno.icon bundle (macOS 26+ Liquid Glass, takes precedence over .icns/Assets.car)
    if [ -d "$RESOURCES/resources/Cteno.icon" ]; then
        cp -R "$RESOURCES/resources/Cteno.icon" "$RESOURCES/Cteno.icon"
        echo "✓ Cteno.icon copied to Contents/Resources"
    fi

    # Copy Assets.car (fallback for macOS 15 and earlier)
    if [ -f "$RESOURCES/resources/Assets.car" ]; then
        cp "$RESOURCES/resources/Assets.car" "$RESOURCES/Assets.car"
        echo "✓ Assets.car copied to Contents/Resources"
    fi

    # Re-sign the app (use Developer ID if available, otherwise ad-hoc)
    if [ -n "$APPLE_SIGNING_IDENTITY" ]; then
        codesign --force --deep --sign "$APPLE_SIGNING_IDENTITY" "$APP_PATH"
        echo "✓ App re-signed with: $APPLE_SIGNING_IDENTITY"
    else
        codesign --force --deep --sign - "$APP_PATH"
        echo "✓ App re-signed (ad-hoc, development only)"
    fi
else
    echo "App not found: $APP_PATH"
    exit 1
fi
