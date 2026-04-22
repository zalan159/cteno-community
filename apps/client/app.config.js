const variant = process.env.APP_ENV || 'development';
const name = {
    development: "Cteno (dev)",
    preview: "Cteno (preview)",
    production: "Cteno智能体"
}[variant];
const bundleId = {
    development: "com.cteno.desktop.dev",
    preview: "com.cteno.desktop.preview",
    production: "com.cteno.desktop"
}[variant];
const LOCAL_ONLY_PLACEHOLDER_SERVER_URL = "";

function requireEnv(name) {
    const value = process.env[name];
    if (!value || !value.trim()) {
        throw new Error(`[config] Missing required env: ${name}`);
    }
    return value.trim();
}

function getConfiguredOrFallback(name, fallback) {
    const value = process.env[name];
    if (value && value.trim()) {
        return value.trim();
    }
    return fallback;
}

const happyServerUrl = getConfiguredOrFallback(
    "EXPO_PUBLIC_HAPPY_SERVER_URL",
    LOCAL_ONLY_PLACEHOLDER_SERVER_URL
);
const expoUpdatesUrl = happyServerUrl
    ? new URL("/api/manifest", happyServerUrl).toString()
    : null;
const appSchemes = ["happy", "cteno"];

export default {
    expo: {
        name,
        slug: "cteno",
        version: "0.1.32",
        runtimeVersion: "18",
        orientation: "default",
        icon: "./app/assets/images/icon.png",
        scheme: appSchemes,
        userInterfaceStyle: "automatic",
        newArchEnabled: true,
        notification: {
            icon: "./app/assets/images/icon-notification.png",
            iosDisplayInForeground: true
        },
        ios: {
            supportsTablet: false,
            bundleIdentifier: bundleId,
            usesAppleSignIn: true,
            config: {
                usesNonExemptEncryption: false
            },
            infoPlist: {
                ...(variant === 'production' ? {
                    NSAppTransportSecurity: {
                        NSAllowsArbitraryLoads: false,
                    }
                } : {}),
                CFBundleURLTypes: [
                    {
                        CFBundleURLName: bundleId,
                        CFBundleURLSchemes: appSchemes,
                    },
                ],
                NSMicrophoneUsageDescription: "Allow $(PRODUCT_NAME) to access your microphone for voice conversations with AI.",
                NSLocalNetworkUsageDescription: "Allow $(PRODUCT_NAME) to find and connect to local devices on your network.",
                NSBonjourServices: ["_http._tcp", "_https._tcp"]
            }
        },
        android: {
            adaptiveIcon: {
                foregroundImage: "./app/assets/images/icon-adaptive.png",
                monochromeImage: "./app/assets/images/icon-monochrome.png",
                backgroundColor: "#18171C"
            },
            permissions: [
                "android.permission.RECORD_AUDIO",
                "android.permission.MODIFY_AUDIO_SETTINGS",
                "android.permission.ACCESS_NETWORK_STATE",
                "android.permission.POST_NOTIFICATIONS",
            ],
            blockedPermissions: [
                "android.permission.ACTIVITY_RECOGNITION"
            ],
            edgeToEdgeEnabled: true,
            package: bundleId,
            intentFilters: [
                {
                    action: "VIEW",
                    category: ["BROWSABLE", "DEFAULT"],
                    data: [
                        {
                            scheme: "cteno",
                            host: "auth",
                            pathPrefix: "/callback",
                        },
                    ],
                },
            ]
        },
        web: {
            bundler: "metro",
            output: "single",
            favicon: "./app/assets/images/favicon.png"
        },
        plugins: [
            [
                "expo-router",
                {
                    root: "./app/app"
                }
            ],
            "expo-updates",
            "expo-asset",
            "expo-localization",
            "expo-mail-composer",
            "expo-web-browser",
            "react-native-vision-camera",
            "@more-tech/react-native-libsodium",
            ["react-native-audio-api", { iosBackgroundMode: false }],
            "@livekit/react-native-expo-plugin",
            "@config-plugins/react-native-webrtc",
            [
                "expo-audio",
                {
                    microphonePermission: "Allow $(PRODUCT_NAME) to access your microphone for voice conversations."
                }
            ],
            [
                "expo-location",
                {
                    locationAlwaysAndWhenInUsePermission: "Allow $(PRODUCT_NAME) to improve AI quality by using your location.",
                    locationAlwaysPermission: "Allow $(PRODUCT_NAME) to improve AI quality by using your location.",
                    locationWhenInUsePermission: "Allow $(PRODUCT_NAME) to improve AI quality by using your location."
                }
            ],
            [
                "expo-calendar",
                {
                    "calendarPermission": "Allow $(PRODUCT_NAME) to access your calendar to improve AI quality."
                }
            ],
            [
                "expo-camera",
                {
                    cameraPermission: "Allow $(PRODUCT_NAME) to access your camera to scan QR codes and share photos with AI.",
                    microphonePermission: "Allow $(PRODUCT_NAME) to access your microphone for voice conversations.",
                    recordAudioAndroid: true
                }
            ],
            [
                "expo-notifications",
                {
                    "enableBackgroundRemoteNotifications": true
                }
            ],
            [
                'expo-splash-screen',
                {
                    ios: {
                        backgroundColor: "#F2F2F7",
                        dark: {
                            backgroundColor: "#1C1C1E",
                        }
                    },
                    android: {
                        image: "./app/assets/images/splash-android-light.png",
                        backgroundColor: "#F5F5F5",
                        dark: {
                            image: "./app/assets/images/splash-android-dark.png",
                            backgroundColor: "#1e1e1e",
                        }
                    }
                }
            ]
        ],
        ...(expoUpdatesUrl ? {
            updates: {
                url: expoUpdatesUrl,
                checkAutomatically: "NEVER",
                requestHeaders: {
                    "expo-channel-name": variant || "development"
                }
            }
        } : {}),
        experiments: {
            typedRoutes: true
        },
        extra: {
            router: {
                root: "./app/app"
            },
            eas: {
                projectId: "93bfe58c-0844-4f5f-842d-966f23302e31"
            },
            app: {
                postHogKey: process.env.EXPO_PUBLIC_POSTHOG_KEY
            }
        },
        owner: "zalan123"
    }
};
