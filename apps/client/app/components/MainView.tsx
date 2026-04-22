import * as React from 'react';
import { View, Pressable } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useSocketStatus, useRealtimeStatus, useAllMachines, useLocalSettingMutable } from '@/sync/storage';
import { useIsTablet } from '@/utils/responsive';
import { useRouter } from 'expo-router';
import { TabBar, TabType } from './TabBar';
import { PersonaView } from './PersonaView';
import { SettingsViewWrapper } from './SettingsViewWrapper';
import { Header } from './navigation/Header';
import { HeaderLogo } from './HeaderLogo';
import { VoiceAssistantStatusBar } from './VoiceAssistantStatusBar';
import { StatusDot } from './StatusDot';
import { Ionicons } from '@expo/vector-icons';
import { Typography } from '@/constants/Typography';
import { t } from '@/text';
import { isUsingCustomServer } from '@/sync/serverConfig';
import { Text } from '@/components/StyledText';
import { isMachineOnline } from '@/utils/machineUtils';
import { getLocalHostInfo } from '@/sync/apiSocket';
import { isTauri } from '@/utils/tauri';


export type SidebarTab = 'persona';

interface MainViewProps {
    variant: 'phone' | 'sidebar';
    sidebarTab?: SidebarTab;
}

const styles = StyleSheet.create((theme) => ({
    phoneContainer: {
        flex: 1,
    },
    sidebarContentContainer: {
        flex: 1,
        flexBasis: 0,
        flexGrow: 1,
    },
    emptyStateContentContainer: {
        flex: 1,
        flexBasis: 0,
        flexGrow: 1,
    },
    titleContainer: {
        flex: 1,
        alignItems: 'center',
    },
    titleText: {
        fontSize: 17,
        color: theme.colors.header.tint,
        fontWeight: '600',
        ...Typography.default('semiBold'),
    },
    statusContainer: {
        flexDirection: 'row',
        alignItems: 'center',
        marginTop: -2,
    },
    statusText: {
        fontSize: 12,
        fontWeight: '500',
        lineHeight: 16,
        ...Typography.default(),
    },
    headerButton: {
        width: 32,
        height: 32,
        alignItems: 'center',
        justifyContent: 'center',
    },
    machineFilterContainer: {
        paddingHorizontal: 12,
        paddingVertical: 6,
        backgroundColor: theme.colors.groupped.background,
        flexDirection: 'row',
        flexWrap: 'wrap',
        gap: 6,
    },
    machineFilterButton: {
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: 10,
        paddingVertical: 4,
        borderRadius: 12,
    },
    machineFilterText: {
        fontSize: 12,
        ...Typography.default('semiBold'),
        flexShrink: 1,
    },
    machineFilterDot: {
        marginRight: 5,
    },
}));

// Tab header configuration (zen excluded as that tab is disabled)
const TAB_TITLES = {
    persona: 'tabs.persona',
    settings: 'tabs.settings',
} as const;

// Active tabs (excludes zen, inbox, sessions which are hidden)
type ActiveTabType = 'persona' | 'settings';

// Machine filter – compact horizontal pills
const MachineFilter = React.memo(() => {
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const [selectedMachineIdFilter, setSelectedMachineIdFilter] = useLocalSettingMutable('selectedMachineIdFilter');
    const [defaultedToLocalMachine, setDefaultedToLocalMachine] = useLocalSettingMutable('defaultedToLocalMachine');

    // Default the filter to the local machine on first launch of a desktop
    // build. Socket sessions from remote machines are intentionally hidden
    // unless the user explicitly clicks "All devices" or picks another
    // machine. We remember the choice so later cross-machine switching
    // sticks across app restarts.
    React.useEffect(() => {
        if (!isTauri()) return;
        if (defaultedToLocalMachine) return;
        let cancelled = false;
        getLocalHostInfo().then((info) => {
            if (cancelled) return;
            if (!info?.machineId) return;
            if (!machines.some((m) => m.id === info.machineId)) return;
            setSelectedMachineIdFilter(info.machineId);
            setDefaultedToLocalMachine(true);
        });
        return () => { cancelled = true; };
    }, [machines, defaultedToLocalMachine, setSelectedMachineIdFilter, setDefaultedToLocalMachine]);

    if (machines.length <= 1) {
        return null;
    }

    return (
        <View style={styles.machineFilterContainer}>
            <Pressable
                onPress={() => setSelectedMachineIdFilter(null)}
                style={[
                    styles.machineFilterButton,
                    selectedMachineIdFilter === null
                        ? { backgroundColor: theme.colors.button.primary.background }
                        : { backgroundColor: theme.colors.surface },
                ]}
            >
                <Text
                    style={[
                        styles.machineFilterText,
                        { color: selectedMachineIdFilter === null ? theme.colors.button.primary.tint : theme.colors.textSecondary },
                    ]}
                    numberOfLines={1}
                >
                    {t('persona.allDevices')}
                </Text>
            </Pressable>
            {machines.map((m) => {
                const selected = m.id === selectedMachineIdFilter;
                const online = isMachineOnline(m);
                const label = m.decryptionFailed
                    ? t('persona.needsKey')
                    : (m.metadata?.displayName || m.metadata?.host || m.id.slice(0, 8));
                return (
                    <Pressable
                        key={m.id}
                        onPress={() => setSelectedMachineIdFilter(m.id)}
                        style={[
                            styles.machineFilterButton,
                            { backgroundColor: selected ? theme.colors.button.primary.background : theme.colors.surface },
                        ]}
                    >
                        <StatusDot
                            color={online ? theme.colors.status.connected : theme.colors.status.disconnected}
                            isPulsing={online}
                            size={5}
                            style={styles.machineFilterDot}
                        />
                        <Text
                            style={[
                                styles.machineFilterText,
                                { color: selected ? theme.colors.button.primary.tint : theme.colors.textSecondary },
                            ]}
                            numberOfLines={1}
                        >
                            {label}
                        </Text>
                    </Pressable>
                );
            })}
        </View>
    );
});

// Header title component with connection status
const HeaderTitle = React.memo(({ activeTab }: { activeTab: ActiveTabType }) => {
    const { theme } = useUnistyles();
    const socketStatus = useSocketStatus();

    const connectionStatus = React.useMemo(() => {
        const { status } = socketStatus;
        switch (status) {
            case 'connected':
                return {
                    color: theme.colors.status.connected,
                    isPulsing: false,
                    text: t('status.connected'),
                };
            case 'connecting':
                return {
                    color: theme.colors.status.connecting,
                    isPulsing: true,
                    text: t('status.connecting'),
                };
            case 'disconnected':
                return {
                    color: theme.colors.status.disconnected,
                    isPulsing: false,
                    text: t('status.disconnected'),
                };
            case 'error':
                return {
                    color: theme.colors.status.error,
                    isPulsing: false,
                    text: t('status.error'),
                };
            default:
                return {
                    color: theme.colors.status.default,
                    isPulsing: false,
                    text: '',
                };
        }
    }, [socketStatus, theme]);

    return (
        <View style={styles.titleContainer}>
            {!!connectionStatus.text && (
                <View style={styles.statusContainer}>
                    <StatusDot
                        color={connectionStatus.color}
                        isPulsing={connectionStatus.isPulsing}
                        size={6}
                        style={{ marginRight: 4 }}
                    />
                    <Text style={[styles.statusText, { color: connectionStatus.color }]}>
                        {connectionStatus.text}
                    </Text>
                </View>
            )}
        </View>
    );
});

// Header right button - varies by tab
const HeaderRight = React.memo(({ activeTab }: { activeTab: ActiveTabType }) => {
    const router = useRouter();
    const { theme } = useUnistyles();
    const isCustomServer = isUsingCustomServer();

    if (activeTab === 'settings') {
        if (!isCustomServer) {
            // Empty view to maintain header centering
            return <View style={styles.headerButton} />;
        }
        return (
            <Pressable
                onPress={() => router.push('/server')}
                hitSlop={15}
                style={styles.headerButton}
            >
                <Ionicons name="server-outline" size={24} color={theme.colors.header.tint} />
            </Pressable>
        );
    }

    if (activeTab === 'persona') {
        // Empty view to maintain header centering
        return <View style={styles.headerButton} />;
    }

    return null;
});

export const MainView = React.memo(({ variant, sidebarTab = 'persona' }: MainViewProps) => {
    const { theme } = useUnistyles();
    const isTablet = useIsTablet();
    const router = useRouter();
    const realtimeStatus = useRealtimeStatus();

    // Tab state management
    // NOTE: Zen tab removed - the feature never got to a useful state
    const [activeTab, setActiveTab] = React.useState<TabType>('persona');

    const handleTabPress = React.useCallback((tab: TabType) => {
        setActiveTab(tab);
    }, []);

    // Tab content — render all tabs but only show the active one.
    // This avoids unmounting/remounting on tab switch and keeps each tab's
    // internal hook state (e.g. usePersonas cache) alive across switches.
    const tabContent = (
        <>
            <View style={{ flex: 1, display: activeTab === 'persona' ? 'flex' : 'none' }}>
                <PersonaView />
            </View>
            <View style={{ flex: 1, display: activeTab === 'settings' ? 'flex' : 'none' }}>
                <SettingsViewWrapper />
            </View>
        </>
    );

    // Sidebar variant — show PersonaView
    if (variant === 'sidebar') {
        return (
            <View style={styles.sidebarContentContainer}>
                <MachineFilter />
                <PersonaView />
            </View>
        );
    }

    // Phone variant
    // Tablet in phone mode - special case (when showing index view on tablets, show empty view)
    if (isTablet) {
        // Just show an empty view on tablets for the index view
        // The sessions list is shown in the sidebar, so the main area should be blank
        return <View style={styles.emptyStateContentContainer} />;
    }

    // Regular phone mode with tabs
    return (
        <>
            <View style={styles.phoneContainer}>
                <View style={{ backgroundColor: theme.colors.groupped.background }}>
                    <Header
                        title={<HeaderTitle activeTab={activeTab as ActiveTabType} />}
                        headerRight={() => <HeaderRight activeTab={activeTab as ActiveTabType} />}
                        headerLeft={() => <HeaderLogo />}
                        headerShadowVisible={false}
                        headerTransparent={true}
                    />
                    {realtimeStatus !== 'disconnected' && (
                        <VoiceAssistantStatusBar variant="full" />
                    )}
                </View>
                {activeTab === 'persona' && <MachineFilter />}
                {tabContent}
            </View>
            <TabBar
                activeTab={activeTab}
                onTabPress={handleTabPress}
            />
        </>
    );
});
