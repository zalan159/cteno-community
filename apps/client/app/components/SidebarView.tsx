import { useSocketStatus, storage } from '@/sync/storage';
import * as React from 'react';
import { View, Pressable } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useRouter } from 'expo-router';
import { useHeaderHeight } from '@/utils/responsive';
import { Typography } from '@/constants/Typography';
import { StatusDot } from './StatusDot';
import { VoiceAssistantStatusBar } from './VoiceAssistantStatusBar';
import { useRealtimeStatus } from '@/sync/storage';
import { MainView } from './MainView';
import { Image } from 'expo-image';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { t } from '@/text';
import { Text } from '@/components/StyledText';
import { useTauriUpdate } from '@/hooks/useTauriUpdate';

const stylesheet = StyleSheet.create((theme, runtime) => ({
    container: {
        flex: 1,
        position: 'relative',
        borderStyle: 'solid',
        backgroundColor: theme.colors.groupped.background,
        borderWidth: StyleSheet.hairlineWidth,
        borderColor: theme.colors.divider,
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: 16,
        backgroundColor: theme.colors.groupped.background,
        position: 'relative',
    },
    logoContainer: {
        width: 32,
    },
    logo: {
        height: 24,
        width: 24,
    },
    titleContainer: {
        position: 'absolute',
        left: 0,
        right: 0,
        flexDirection: 'column',
        alignItems: 'center',
        pointerEvents: 'none',
    },
    titleContainerLeft: {
        flex: 1,
        flexDirection: 'column',
        alignItems: 'flex-start',
        marginLeft: 8,
        justifyContent: 'center',
    },
    titleText: {
        fontSize: 17,
        fontWeight: '600',
        color: theme.colors.header.tint,
        ...Typography.default('semiBold'),
    },
    statusContainer: {
        flexDirection: 'row',
        alignItems: 'center',
        marginTop: -2,
    },
    statusDot: {
        marginRight: 4,
    },
    statusText: {
        fontSize: 11,
        fontWeight: '500',
        lineHeight: 16,
        ...Typography.default(),
    },
    rightContainer: {
        marginLeft: 'auto',
        alignItems: 'flex-end',
        flexDirection: 'row',
        gap: 8,
    },
    settingsButton: {
        color: theme.colors.header.tint,
    },
    notificationButton: {
        position: 'relative',
    },
    badge: {
        position: 'absolute',
        top: -4,
        right: -4,
        backgroundColor: theme.colors.status.error,
        borderRadius: 8,
        minWidth: 16,
        height: 16,
        paddingHorizontal: 4,
        justifyContent: 'center',
        alignItems: 'center',
    },
    badgeText: {
        color: '#FFFFFF',
        fontSize: 10,
        ...Typography.default('semiBold'),
    },
    // Status colors
    statusConnected: {
        color: theme.colors.status.connected,
    },
    statusConnecting: {
        color: theme.colors.status.connecting,
    },
    statusDisconnected: {
        color: theme.colors.status.disconnected,
    },
    statusError: {
        color: theme.colors.status.error,
    },
    statusDefault: {
        color: theme.colors.status.default,
    },
    indicatorDot: {
        position: 'absolute',
        top: 0,
        right: -2,
        width: 6,
        height: 6,
        borderRadius: 3,
        backgroundColor: theme.colors.text,
    },
    updateBadge: {
        position: 'absolute',
        top: -2,
        right: -6,
        backgroundColor: '#FF3B30',
        borderRadius: 7,
        minWidth: 14,
        height: 14,
        paddingHorizontal: 3,
        alignItems: 'center',
        justifyContent: 'center',
    },
    updateBadgeText: {
        color: '#FFFFFF',
        fontSize: 9,
        ...Typography.default('semiBold'),
    },
    resizeHandle: {
        position: 'absolute',
        top: 0,
        right: -10,
        bottom: 0,
        width: 20,
        zIndex: 30,
        pointerEvents: 'none',
    } as any,
    resizeHandleLine: {
        position: 'absolute',
        top: 0,
        bottom: 0,
        left: 9,
        width: 2,
    },
}));

interface SidebarViewProps {
    sidebarWidth: number;
    onEdgeChange?: (clientRight: number) => void;
    showResizeHandle?: boolean;
    isResizing?: boolean;
}

export const SidebarView = React.memo(({ sidebarWidth, onEdgeChange, showResizeHandle = false, isResizing = false }: SidebarViewProps) => {
    const styles = stylesheet;
    const { theme } = useUnistyles();
    const safeArea = useSafeAreaInsets();
    const router = useRouter();
    const headerHeight = useHeaderHeight();
    const socketStatus = useSocketStatus();
    const realtimeStatus = useRealtimeStatus();
    const containerRef = React.useRef<any>(null);

    // Compute connection status once per render (theme-reactive, no stale memoization)
    const connectionStatus = (() => {
        const { status } = socketStatus;
        switch (status) {
            case 'connected':
                return {
                    color: styles.statusConnected.color,
                    isPulsing: false,
                    text: t('status.connected'),
                    textColor: styles.statusConnected.color
                };
            case 'connecting':
                return {
                    color: styles.statusConnecting.color,
                    isPulsing: true,
                    text: t('status.connecting'),
                    textColor: styles.statusConnecting.color
                };
            case 'disconnected':
                return {
                    color: styles.statusDisconnected.color,
                    isPulsing: false,
                    text: t('status.disconnected'),
                    textColor: styles.statusDisconnected.color
                };
            case 'error':
                return {
                    color: styles.statusError.color,
                    isPulsing: false,
                    text: t('status.error'),
                    textColor: styles.statusError.color
                };
            default:
                return {
                    color: styles.statusDefault.color,
                    isPulsing: false,
                    text: '',
                    textColor: styles.statusDefault.color
                };
        }
    })();

    // Keep the status text left-aligned once the sidebar gets tight enough to risk header overlap.
    const shouldLeftJustify = sidebarWidth < 340;

    const tauriUpdate = useTauriUpdate();

    React.useEffect(() => {
        if (typeof window === 'undefined' || !onEdgeChange) {
            return;
        }

        const element = containerRef.current as HTMLElement | null;
        if (!element || typeof element.getBoundingClientRect !== 'function') {
            return;
        }

        let frameId = 0;
        const reportEdge = () => {
            frameId = 0;
            const rect = element.getBoundingClientRect();
            onEdgeChange(rect.right);
        };

        const scheduleReport = () => {
            if (frameId) {
                cancelAnimationFrame(frameId);
            }
            frameId = requestAnimationFrame(reportEdge);
        };

        scheduleReport();

        const resizeObserver = typeof ResizeObserver !== 'undefined'
            ? new ResizeObserver(scheduleReport)
            : null;
        resizeObserver?.observe(element);
        window.addEventListener('resize', scheduleReport);

        return () => {
            if (frameId) {
                cancelAnimationFrame(frameId);
            }
            resizeObserver?.disconnect();
            window.removeEventListener('resize', scheduleReport);
        };
    }, [onEdgeChange, sidebarWidth]);


    // Title content used in both centered and left-justified modes (DRY)
    const titleContent = (
        <>
            {!!connectionStatus.text && (
                <View style={styles.statusContainer}>
                    <StatusDot
                        color={connectionStatus.color}
                        isPulsing={connectionStatus.isPulsing}
                        size={6}
                        style={styles.statusDot}
                    />
                    <Text style={[styles.statusText, { color: connectionStatus.textColor }]}>
                        {connectionStatus.text}
                    </Text>
                </View>
            )}
        </>
    );

    return (
        <>
            <View ref={containerRef} style={[styles.container, { paddingTop: safeArea.top }]}>
                <View style={[styles.header, { height: headerHeight }]}>
                    {/* Logo - with update badge */}
                    <Pressable
                        style={styles.logoContainer}
                        onPress={tauriUpdate.available ? () => {
                            storage.getState().setShowDesktopUpdateModal(true);
                        } : undefined}
                        disabled={!tauriUpdate.available}
                    >
                        <Image
                            source={theme.dark ? require('@/assets/images/logo-white.png') : require('@/assets/images/logo-black.png')}
                            contentFit="contain"
                            style={[styles.logo, { height: 24, width: 24 }]}
                        />
                        {tauriUpdate.available && (
                            <View style={styles.updateBadge}>
                                <Text style={styles.updateBadgeText}>NEW</Text>
                            </View>
                        )}
                    </Pressable>

                    {/* Left-justified title - in document flow, prevents overlap */}
                    {shouldLeftJustify && (
                        <View style={styles.titleContainerLeft}>
                            {titleContent}
                        </View>
                    )}

                    {/* Navigation icons */}
                    <View style={styles.rightContainer}>
                        <Pressable
                            onPress={() => router.push('/settings')}
                            hitSlop={15}
                        >
                            <Image
                                source={require('@/assets/images/brutalist/Brutalism 9.png')}
                                contentFit="contain"
                                style={[{ width: 32, height: 32 }]}
                                tintColor={theme.colors.header.tint}
                            />
                        </Pressable>
                    </View>

                    {/* Centered title - absolute positioned over full header */}
                    {!shouldLeftJustify && (
                        <View style={styles.titleContainer}>
                            {titleContent}
                        </View>
                    )}
                </View>
                {realtimeStatus !== 'disconnected' && (
                    <VoiceAssistantStatusBar variant="sidebar" />
                )}
                <MainView variant="sidebar" />
                {showResizeHandle && (
                    <View style={styles.resizeHandle}>
                        <View
                            style={[
                                styles.resizeHandleLine,
                                {
                                    backgroundColor: isResizing
                                        ? theme.colors.textSecondary
                                        : theme.colors.divider,
                                },
                            ]}
                        />
                    </View>
                )}
            </View>
        </>
    )
});
