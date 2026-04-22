import 'react-native-quick-base64';
import '../theme.css';
import * as React from 'react';
import * as SplashScreen from 'expo-splash-screen';
import * as Fonts from 'expo-font';
import * as Notifications from 'expo-notifications';
import {
    Feather,
    FontAwesome,
    FontAwesome5,
    Ionicons,
    MaterialCommunityIcons,
    MaterialIcons,
    Octicons,
} from '@expo/vector-icons';
import { AuthCredentials, TokenStorage } from '@/auth/tokenStorage';
import { AuthProvider } from '@/auth/AuthContext';
import { isDesktopLocalModeEnabled, AnonymousAuthBoot } from '@/auth/local_mode';
import { DarkTheme, DefaultTheme, ThemeProvider } from '@react-navigation/native';
import { KeyboardProvider } from 'react-native-keyboard-controller';
import { initialWindowMetrics, SafeAreaProvider, useSafeAreaInsets } from 'react-native-safe-area-context';
import { GestureHandlerRootView } from 'react-native-gesture-handler';
import { SidebarNavigator } from '@/components/SidebarNavigator';
import sodium from '@/encryption/libsodium.lib';
import { View, Platform } from 'react-native';
import { ModalProvider } from '@/modal';
import { PostHogProvider } from 'posthog-react-native';
import { tracking } from '@/track/tracking';
import { syncInitLocalMode, syncRestore } from '@/sync/sync';
import '@/utils/debugEncryption'; // 加载调试工具
import { useTrackScreens } from '@/track/useTrackScreens';
import { RealtimeProvider } from '@/realtime/RealtimeProvider';
import { FaviconPermissionIndicator } from '@/components/web/FaviconPermissionIndicator';
import { CommandPaletteProvider } from '@/components/CommandPalette/CommandPaletteProvider';
import { DesktopUpdateModal } from '@/components/DesktopUpdateModal';
import { useTauriUpdate } from '@/hooks/useTauriUpdate';
import { storage as storageStore } from '@/sync/storage';
import { StatusBarProvider } from '@/components/StatusBarProvider';
// import * as SystemUI from 'expo-system-ui';
import { monkeyPatchConsoleForRemoteLoggingForFasterAiAutoDebuggingOnlyInLocalBuilds } from '@/utils/remoteLogger';
import { frontendLog, isTauri } from '@/utils/tauri';
import { useUnistyles } from 'react-native-unistyles';
import { AsyncLock } from '@/utils/lock';
import { usePathname } from 'expo-router';

// Configure notification handler for foreground notifications
Notifications.setNotificationHandler({
    handleNotification: async () => ({
        shouldShowAlert: true,
        shouldPlaySound: true,
        shouldSetBadge: true,
        shouldShowBanner: true,
        shouldShowList: true,
    }),
});

// Setup Android notification channel (required for Android 8.0+)
if (Platform.OS === 'android') {
    Notifications.setNotificationChannelAsync('default', {
        name: 'Default',
        importance: Notifications.AndroidImportance.MAX,
        vibrationPattern: [0, 250, 250, 250],
        lightColor: '#FF231F7C',
    });
}

export {
    // Catch any errors thrown by the Layout component.
    ErrorBoundary,
} from 'expo-router';

// Configure splash screen
SplashScreen.setOptions({
    fade: true,
    duration: 300,
})
SplashScreen.preventAutoHideAsync();

// Set window background color - now handled by Unistyles
// SystemUI.setBackgroundColorAsync('white');

// NEVER ENABLE REMOTE LOGGING IN PRODUCTION
// This is for local debugging with AI only
// So AI will have all the logs easily accessible in one file for analysis
if (!!process.env.PUBLIC_EXPO_DANGEROUSLY_LOG_TO_SERVER_FOR_AI_AUTO_DEBUGGING) {
    monkeyPatchConsoleForRemoteLoggingForFasterAiAutoDebuggingOnlyInLocalBuilds()
}

// Component to apply horizontal safe area padding
function DesktopUpdateModalContainer() {
    const tauriUpdate = useTauriUpdate();
    const showModal = storageStore((s) => s.showDesktopUpdateModal);
    const setShowModal = storageStore((s) => s.setShowDesktopUpdateModal);

    if (!tauriUpdate.available) return null;

    const handleConfirm = () => {
        if (tauriUpdate.progress === 100) {
            tauriUpdate.relaunchApp();
        } else {
            tauriUpdate.startDownload();
        }
    };

    return (
        <DesktopUpdateModal
            visible={showModal}
            onClose={() => setShowModal(false)}
            version={tauriUpdate.version}
            notes={tauriUpdate.notes}
            downloading={tauriUpdate.downloading}
            progress={tauriUpdate.progress}
            error={tauriUpdate.error}
            onConfirm={handleConfirm}
        />
    );
}

function HorizontalSafeAreaWrapper({ children }: { children: React.ReactNode }) {
    const insets = useSafeAreaInsets();
    return (
        <View style={{
            flex: 1,
            paddingLeft: insets.left,
            paddingRight: insets.right
        }}>
            {children}
        </View>
    );
}

function DesktopAttentionSync() {
    const pathname = usePathname();
    const [appActive, setAppActive] = React.useState(true);
    const cachedPersonas = storageStore((state) => state.cachedPersonas);

    const activeSessionId = React.useMemo(() => {
        // Treat only the exact chat routes as "conversation in foreground".
        // Nested routes under /session/:id (e.g. side panels/tools) should
        // not suppress completion notifications.
        const sessionMatch = pathname.match(/^\/session\/([^/]+)$/);
        if (sessionMatch?.[1]) {
            return decodeURIComponent(sessionMatch[1]);
        }

        // Persona route foregrounds the persona's chat session.
        const personaMatch = pathname.match(/^\/persona\/([^/]+)$/);
        if (personaMatch?.[1]) {
            const personaId = decodeURIComponent(personaMatch[1]);
            const persona = cachedPersonas.find((item) => item.id === personaId);
            return persona?.chatSessionId ?? null;
        }

        return null;
    }, [cachedPersonas, pathname]);

    React.useEffect(() => {
        if (!isTauri() || typeof window === 'undefined' || typeof document === 'undefined') {
            return;
        }

        const syncAppActivity = () => {
            setAppActive(document.visibilityState === 'visible' && document.hasFocus());
        };

        syncAppActivity();
        window.addEventListener('focus', syncAppActivity);
        window.addEventListener('blur', syncAppActivity);
        document.addEventListener('visibilitychange', syncAppActivity);

        return () => {
            window.removeEventListener('focus', syncAppActivity);
            window.removeEventListener('blur', syncAppActivity);
            document.removeEventListener('visibilitychange', syncAppActivity);
        };
    }, []);

    React.useEffect(() => {
        if (!isTauri()) {
            return;
        }

        import('@tauri-apps/api/core')
            .then(({ invoke }) =>
                invoke('update_attention_state', {
                    activeSessionId,
                    appActive,
                }),
            )
            .catch((error) => {
                frontendLog(
                    `[DesktopAttentionSync] Failed to update attention state: ${String(error)}`,
                    'warn',
                );
            });
    }, [activeSessionId, appActive]);

    return null;
}

let lock = new AsyncLock();
let loaded = false;
const preloadedIconFonts = {
    ...Feather.font,
    ...FontAwesome.font,
    ...FontAwesome5.font,
    ...Ionicons.font,
    ...MaterialCommunityIcons.font,
    ...MaterialIcons.font,
    ...Octicons.font,
};

async function loadFonts() {
    await lock.inLock(async () => {
        if (loaded) {
            return;
        }
        loaded = true;
        // Check if running in Tauri
        const isTauri = Platform.OS === 'web' &&
            typeof window !== 'undefined' &&
            (window as any).__TAURI_INTERNALS__ !== undefined;

        if (!isTauri) {
            // Normal font loading for non-Tauri environments (native and regular web)
            try {
            await Fonts.loadAsync({
                // Keep existing font
                SpaceMono: require('@/assets/fonts/SpaceMono-Regular.ttf'),

                // IBM Plex Sans family
                'IBMPlexSans-Regular': require('@/assets/fonts/IBMPlexSans-Regular.ttf'),
                'IBMPlexSans-Italic': require('@/assets/fonts/IBMPlexSans-Italic.ttf'),
                'IBMPlexSans-SemiBold': require('@/assets/fonts/IBMPlexSans-SemiBold.ttf'),

                // IBM Plex Mono family  
                'IBMPlexMono-Regular': require('@/assets/fonts/IBMPlexMono-Regular.ttf'),
                'IBMPlexMono-Italic': require('@/assets/fonts/IBMPlexMono-Italic.ttf'),
                'IBMPlexMono-SemiBold': require('@/assets/fonts/IBMPlexMono-SemiBold.ttf'),

                // Bricolage Grotesque  
                'BricolageGrotesque-Bold': require('@/assets/fonts/BricolageGrotesque-Bold.ttf'),

                ...preloadedIconFonts,
            });
            } catch (e) {
                console.warn('[fonts] Font loading timeout (safe to ignore on hot reload):', e);
            }
        } else {
            // For Tauri, skip Font Face Observer as fonts are loaded via CSS
            console.log('Do not wait for fonts to load');
            (async () => {
                try {
                    await Fonts.loadAsync({
                        // Keep existing font
                        SpaceMono: require('@/assets/fonts/SpaceMono-Regular.ttf'),

                        // IBM Plex Sans family
                        'IBMPlexSans-Regular': require('@/assets/fonts/IBMPlexSans-Regular.ttf'),
                        'IBMPlexSans-Italic': require('@/assets/fonts/IBMPlexSans-Italic.ttf'),
                        'IBMPlexSans-SemiBold': require('@/assets/fonts/IBMPlexSans-SemiBold.ttf'),

                        // IBM Plex Mono family  
                        'IBMPlexMono-Regular': require('@/assets/fonts/IBMPlexMono-Regular.ttf'),
                        'IBMPlexMono-Italic': require('@/assets/fonts/IBMPlexMono-Italic.ttf'),
                        'IBMPlexMono-SemiBold': require('@/assets/fonts/IBMPlexMono-SemiBold.ttf'),

                        // Bricolage Grotesque  
                        'BricolageGrotesque-Bold': require('@/assets/fonts/BricolageGrotesque-Bold.ttf'),

                        ...preloadedIconFonts,
                    });
                } catch (e) {
                    // Ignore
                }
            })();
        }
    });
}

export default function RootLayout() {
    const { theme } = useUnistyles();
    const navigationTheme = React.useMemo(() => {
        if (theme.dark) {
            return {
                ...DarkTheme,
                colors: {
                    ...DarkTheme.colors,
                    background: theme.colors.groupped.background,
                }
            }
        }
        return {
            ...DefaultTheme,
            colors: {
                ...DefaultTheme.colors,
                background: theme.colors.groupped.background,
            }
        };
    }, [theme.dark]);

    //
    // Init sequence
    //
    const [initState, setInitState] = React.useState<{
        credentials: AuthCredentials | null;
        localMode: boolean;
    } | null>(null);
    const hasStoredCredentials = React.useMemo(() => TokenStorage.peekCredentials() !== null, []);
    React.useEffect(() => {
        (async () => {
            try {
                await loadFonts();
                await sodium.ready;
                const credentials = await TokenStorage.getCredentials();
                const localMode = !credentials && isDesktopLocalModeEnabled();
                console.log('credentials', credentials);
                if (credentials) {
                    await syncRestore(credentials);
                } else if (localMode) {
                    await syncInitLocalMode();
                }

                setInitState({ credentials, localMode });
            } catch (error) {
                console.error('Error initializing:', error);
            }
        })();
    }, []);

    React.useEffect(() => {
        if (initState) {
            setTimeout(() => {
                SplashScreen.hideAsync();
            }, 100);
        }
    }, [initState]);


    // Track the screens
    useTrackScreens()

    //
    // Not inited
    //

    if (!initState) {
        if (!hasStoredCredentials && isDesktopLocalModeEnabled()) {
            return <AnonymousAuthBoot />;
        }
        return null;
    }

    //
    // Boot
    //

    let providers = (
        <SafeAreaProvider initialMetrics={initialWindowMetrics}>
            <KeyboardProvider>
                <GestureHandlerRootView style={{ flex: 1 }}>
                    <AuthProvider
                        initialCredentials={initState.credentials}
                        initialLocalMode={initState.localMode}
                    >
                        <ThemeProvider value={navigationTheme}>
                            <StatusBarProvider />
                            <ModalProvider>
                                <CommandPaletteProvider>
                                    <RealtimeProvider>
                                        <HorizontalSafeAreaWrapper>
                                            <SidebarNavigator />
                                        </HorizontalSafeAreaWrapper>
                                        <DesktopUpdateModalContainer />
                                    </RealtimeProvider>
                                </CommandPaletteProvider>
                            </ModalProvider>
                        </ThemeProvider>
                    </AuthProvider>
                </GestureHandlerRootView>
            </KeyboardProvider>
        </SafeAreaProvider>
    );
    if (tracking) {
        providers = (
            <PostHogProvider client={tracking}>
                {providers}
            </PostHogProvider>
        );
    }

    return (
        <>
            <FaviconPermissionIndicator />
            <DesktopAttentionSync />
            {providers}
        </>
    );
}
