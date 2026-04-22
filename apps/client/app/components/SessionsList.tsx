import React from 'react';
import { View, Pressable, FlatList, Platform } from 'react-native';
import { Swipeable } from 'react-native-gesture-handler';
import { Text } from '@/components/StyledText';
import { usePathname } from 'expo-router';
import { SessionListViewItem } from '@/sync/storage';
import { Ionicons } from '@expo/vector-icons';
import { getSessionName, useSessionStatus, getSessionSubtitle, getSessionAvatarId } from '@/utils/sessionUtils';
import { Avatar } from './Avatar';
import { ActiveSessionsGroup } from './ActiveSessionsGroup';
import { ActiveSessionsGroupCompact } from './ActiveSessionsGroupCompact';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useSetting } from '@/sync/storage';
import { useVisibleSessionListViewData } from '@/hooks/useVisibleSessionListViewData';
import { Typography } from '@/constants/Typography';
import { Session } from '@/sync/storageTypes';
import { StatusDot } from './StatusDot';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useIsTablet } from '@/utils/responsive';
import { requestReview } from '@/utils/requestReview';
import { UpdateBanner } from './UpdateBanner';
import { layout } from './layout';
import { useNavigateToSession } from '@/hooks/useNavigateToSession';
import { t } from '@/text';
import { useRouter } from 'expo-router';
import { Item } from './Item';
import { ItemGroup } from './ItemGroup';
import { useHappyAction } from '@/hooks/useHappyAction';
import { sessionDelete } from '@/sync/ops';
import { HappyError } from '@/utils/errors';
import { Modal } from '@/modal';
import { VendorName } from '@/sync/ops';
import { inferSessionVendor } from '@/hooks/useCapability';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        flexDirection: 'row',
        justifyContent: 'center',
        alignItems: 'stretch',
        backgroundColor: theme.colors.groupped.background,
    },
    contentContainer: {
        flex: 1,
        maxWidth: layout.maxWidth,
    },
    headerSection: {
        backgroundColor: theme.colors.groupped.background,
        paddingHorizontal: 24,
        paddingTop: 20,
        paddingBottom: 8,
    },
    headerText: {
        fontSize: 14,
        fontWeight: '600',
        color: theme.colors.groupped.sectionTitle,
        letterSpacing: 0.1,
        ...Typography.default('semiBold'),
    },
    projectGroup: {
        paddingHorizontal: 16,
        paddingVertical: 10,
        backgroundColor: theme.colors.surface,
    },
    projectGroupTitle: {
        fontSize: 13,
        fontWeight: '600',
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    projectGroupSubtitle: {
        fontSize: 11,
        color: theme.colors.textSecondary,
        marginTop: 2,
        ...Typography.default(),
    },
    sessionItem: {
        height: 88,
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: 16,
        width: '100%',
        alignSelf: 'stretch',
        backgroundColor: theme.colors.surface,
    },
    sessionItemWithHoverActions: {
        paddingRight: 104,
    },
    sessionItemWithConfirmActions: {
        paddingRight: 168,
    },
    sessionItemContainer: {
        marginHorizontal: 16,
        marginBottom: 1,
        overflow: 'hidden',
    },
    sessionItemFirst: {
        borderTopLeftRadius: 12,
        borderTopRightRadius: 12,
    },
    sessionItemLast: {
        borderBottomLeftRadius: 12,
        borderBottomRightRadius: 12,
    },
    sessionItemSingle: {
        borderRadius: 12,
    },
    sessionItemContainerFirst: {
        borderTopLeftRadius: 12,
        borderTopRightRadius: 12,
    },
    sessionItemContainerLast: {
        borderBottomLeftRadius: 12,
        borderBottomRightRadius: 12,
        marginBottom: 12,
    },
    sessionItemContainerSingle: {
        borderRadius: 12,
        marginBottom: 12,
    },
    sessionItemSelected: {
        backgroundColor: theme.colors.surfaceSelected,
    },
    sessionContent: {
        flex: 1,
        marginLeft: 16,
        justifyContent: 'center',
    },
    sessionTitleRow: {
        flexDirection: 'row',
        alignItems: 'center',
        marginBottom: 2,
    },
    sessionTitle: {
        fontSize: 15,
        fontWeight: '500',
        flex: 1,
        ...Typography.default('semiBold'),
    },
    sessionTitleConnected: {
        color: theme.colors.text,
    },
    sessionTitleDisconnected: {
        color: theme.colors.textSecondary,
    },
    sessionSubtitle: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        marginBottom: 4,
        ...Typography.default(),
    },
    statusRow: {
        flexDirection: 'row',
        alignItems: 'center',
    },
    statusDotContainer: {
        alignItems: 'center',
        justifyContent: 'center',
        height: 16,
        marginTop: 2,
        marginRight: 4,
    },
    statusText: {
        fontSize: 12,
        fontWeight: '500',
        lineHeight: 16,
        ...Typography.default(),
    },
    avatarContainer: {
        position: 'relative',
        width: 48,
        height: 48,
    },
    draftIconContainer: {
        position: 'absolute',
        bottom: -2,
        right: -2,
        width: 18,
        height: 18,
        alignItems: 'center',
        justifyContent: 'center',
    },
    draftIconOverlay: {
        color: theme.colors.textSecondary,
    },
    swipeAction: {
        width: 112,
        height: '100%',
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: theme.colors.status.error,
    },
    swipeActionText: {
        marginTop: 4,
        fontSize: 12,
        color: '#FFFFFF',
        textAlign: 'center',
        ...Typography.default('semiBold'),
    },
    inlineActionButton: {
        width: 32,
        height: 32,
        borderRadius: 16,
        alignItems: 'center',
        justifyContent: 'center',
    },
    inlineActionButtonPressed: {
        backgroundColor: theme.colors.surfaceHighest,
    },
    inlineActionsRail: {
        position: 'absolute',
        right: 8,
        top: 0,
        bottom: 0,
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    inlineConfirmButton: {
        height: 30,
        paddingHorizontal: 10,
        borderRadius: 999,
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: theme.colors.surfaceHighest,
    },
    inlineConfirmDeleteButton: {
        minWidth: 68,
        paddingHorizontal: 12,
        backgroundColor: theme.colors.deleteAction,
    },
    inlineConfirmText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
    inlineConfirmDeleteText: {
        color: '#FFFFFF',
    },
    vendorFilterBar: {
        flexDirection: 'row',
        paddingHorizontal: 16,
        paddingVertical: 8,
        gap: 8,
        backgroundColor: theme.colors.groupped.background,
    },
    vendorFilterChip: {
        paddingHorizontal: 12,
        paddingVertical: 6,
        borderRadius: 999,
        backgroundColor: theme.colors.surface,
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    vendorFilterChipActive: {
        backgroundColor: theme.colors.button.primary.background,
        borderColor: theme.colors.button.primary.background,
    },
    vendorFilterChipLabel: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
    vendorFilterChipLabelActive: {
        color: '#FFFFFF',
    },
    vendorBadge: {
        paddingHorizontal: 6,
        paddingVertical: 2,
        borderRadius: 6,
        marginLeft: 6,
        backgroundColor: theme.colors.surfaceHighest,
    },
    vendorBadgeLabel: {
        fontSize: 10,
        lineHeight: 12,
        color: theme.colors.textSecondary,
        ...Typography.default('semiBold'),
    },
}));

type VendorFilter = 'all' | VendorName;

const VENDOR_FILTER_ITEMS: { value: VendorFilter; label: string }[] = [
    { value: 'all', label: 'All' },
    { value: 'cteno', label: 'Cteno' },
    { value: 'claude', label: 'Claude' },
    { value: 'codex', label: 'Codex' },
];

function VendorFilterBar({
    value,
    onChange,
}: {
    value: VendorFilter;
    onChange: (v: VendorFilter) => void;
}) {
    const styles = stylesheet;
    return (
        <View style={styles.vendorFilterBar}>
            {VENDOR_FILTER_ITEMS.map((item) => {
                const active = item.value === value;
                return (
                    <Pressable
                        key={item.value}
                        onPress={() => onChange(item.value)}
                        style={[
                            styles.vendorFilterChip,
                            active && styles.vendorFilterChipActive,
                        ]}
                    >
                        <Text
                            style={[
                                styles.vendorFilterChipLabel,
                                active && styles.vendorFilterChipLabelActive,
                            ]}
                        >
                            {item.label}
                        </Text>
                    </Pressable>
                );
            })}
        </View>
    );
}

function VendorBadge({ vendor }: { vendor: VendorName }) {
    const styles = stylesheet;
    const label =
        vendor === 'cteno' ? 'Cteno' :
        vendor === 'claude' ? 'Claude' :
        vendor === 'gemini' ? 'Gemini' :
        'Codex';
    return (
        <View style={styles.vendorBadge}>
            <Text style={styles.vendorBadgeLabel}>{label}</Text>
        </View>
    );
}

export function SessionsList() {
    const styles = stylesheet;
    const safeArea = useSafeAreaInsets();
    const data = useVisibleSessionListViewData();
    const pathname = usePathname();
    const isTablet = useIsTablet();
    const navigateToSession = useNavigateToSession();
    const compactActiveSessions = useSetting('compactActiveSessions');
    const router = useRouter();
    const selectable = isTablet;
    const experiments = useSetting('experiments');
    const [vendorFilter, setVendorFilter] = React.useState<VendorFilter>('all');

    // Apply the vendor filter before we decorate with `selected` so the
    // list keyExtractor stays consistent. `session` rows that don't match
    // are dropped; other rows (headers, active groups, project groups)
    // pass through.
    const filteredData = React.useMemo(() => {
        if (!data) return data;
        if (vendorFilter === 'all') return data;
        return data.filter((item) => {
            if (item.type !== 'session') return true;
            return inferSessionVendor(item.session) === vendorFilter;
        });
    }, [data, vendorFilter]);

    const dataWithSelected = selectable ? React.useMemo(() => {
        return filteredData?.map(item => ({
            ...item,
            selected: pathname.startsWith(`/session/${item.type === 'session' ? item.session.id : ''}`)
        }));
    }, [filteredData, pathname]) : filteredData;

    // Request review
    React.useEffect(() => {
        if (data && data.length > 0) {
            requestReview();
        }
    }, [data && data.length > 0]);

    // Early return if no data yet
    if (!data) {
        return (
            <View style={styles.container} />
        );
    }

    const keyExtractor = React.useCallback((item: SessionListViewItem & { selected?: boolean }, index: number) => {
        switch (item.type) {
            case 'header': return `header-${item.title}-${index}`;
            case 'active-sessions': return 'active-sessions';
            case 'project-group': return `project-group-${item.machine.id}-${item.displayPath}-${index}`;
            case 'session': return `session-${item.session.id}`;
        }
    }, []);

    const renderItem = React.useCallback(({ item, index }: { item: SessionListViewItem & { selected?: boolean }, index: number }) => {
        switch (item.type) {
            case 'header':
                return (
                    <View style={styles.headerSection}>
                        <Text style={styles.headerText}>
                            {item.title}
                        </Text>
                    </View>
                );

            case 'active-sessions':
                // Extract just the session ID from pathname (e.g., /session/abc123/file -> abc123)
                let selectedId: string | undefined;
                if (isTablet && pathname.startsWith('/session/')) {
                    const parts = pathname.split('/');
                    selectedId = parts[2]; // parts[0] is empty, parts[1] is 'session', parts[2] is the ID
                }

                const ActiveComponent = compactActiveSessions ? ActiveSessionsGroupCompact : ActiveSessionsGroup;
                return (
                    <ActiveComponent
                        sessions={item.sessions}
                        selectedSessionId={selectedId}
                    />
                );

            case 'project-group':
                return (
                    <View style={styles.projectGroup}>
                        <Text style={styles.projectGroupTitle}>
                            {item.displayPath}
                        </Text>
                        <Text style={styles.projectGroupSubtitle}>
                            {item.machine.decryptionFailed
                                ? '🔐 需要导入设备密钥'
                                : (item.machine.metadata?.displayName || item.machine.metadata?.host || item.machine.id)
                            }
                        </Text>
                    </View>
                );

            case 'session':
                // Determine card styling based on position within date group
                const prevItem = index > 0 && dataWithSelected ? dataWithSelected[index - 1] : null;
                const nextItem = index < (dataWithSelected?.length || 0) - 1 && dataWithSelected ? dataWithSelected[index + 1] : null;

                const isFirst = prevItem?.type === 'header';
                const isLast = nextItem?.type === 'header' || nextItem == null || nextItem?.type === 'active-sessions';
                const isSingle = isFirst && isLast;

                return (
                    <SessionItem
                        session={item.session}
                        selected={item.selected}
                        isFirst={isFirst}
                        isLast={isLast}
                        isSingle={isSingle}
                    />
                );
        }
    }, [pathname, dataWithSelected, compactActiveSessions]);


    // Remove this section as we'll use FlatList for all items now


    const HeaderComponent = React.useCallback(() => {
        return (
            <VendorFilterBar
                value={vendorFilter}
                onChange={setVendorFilter}
            />
        );
    }, [vendorFilter]);

    // Footer removed - all sessions now shown inline

    return (
        <View style={styles.container}>
            <View style={styles.contentContainer}>
                <FlatList
                    data={dataWithSelected}
                    renderItem={renderItem}
                    keyExtractor={keyExtractor}
                    contentContainerStyle={{ paddingBottom: safeArea.bottom + 128, maxWidth: layout.maxWidth }}
                    ListHeaderComponent={HeaderComponent}
                />
            </View>
        </View>
    );
}

// Sub-component that handles session message logic
const SessionItem = React.memo(({ session, selected, isFirst, isLast, isSingle }: {
    session: Session;
    selected?: boolean;
    isFirst?: boolean;
    isLast?: boolean;
    isSingle?: boolean;
}) => {
    const styles = stylesheet;
    const sessionStatus = useSessionStatus(session);
    const sessionName = getSessionName(session);
    const sessionSubtitle = getSessionSubtitle(session);
    const navigateToSession = useNavigateToSession();
    const isTablet = useIsTablet();
    const swipeableRef = React.useRef<Swipeable | null>(null);
    const swipeEnabled = Platform.OS !== 'web';
    const [hovered, setHovered] = React.useState(false);
    const [confirmingDelete, setConfirmingDelete] = React.useState(false);
    const hideHoverTimeoutRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

    const [deletingSession, performDelete] = useHappyAction(async () => {
        const result = await sessionDelete(session.id);
        if (!result.success) {
            throw new HappyError(result.message || t('sessionInfo.failedToDeleteSession'), false);
        }
    });

    const handleDelete = React.useCallback(() => {
        swipeableRef.current?.close();
        Modal.alert(
            t('sessionInfo.deleteSession'),
            t('sessionInfo.deleteSessionWarning'),
            [
                { text: t('common.cancel'), style: 'cancel' },
                {
                    text: t('sessionInfo.deleteSession'),
                    style: 'destructive',
                    onPress: performDelete
                }
            ]
        );
    }, [performDelete]);
    const avatarId = React.useMemo(() => {
        return getSessionAvatarId(session);
    }, [session]);

    const showHover = React.useCallback(() => {
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
            hideHoverTimeoutRef.current = null;
        }
        setHovered(true);
    }, []);

    const scheduleHideHover = React.useCallback(() => {
        if (confirmingDelete) return;
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
        }
        hideHoverTimeoutRef.current = setTimeout(() => {
            setHovered(false);
            hideHoverTimeoutRef.current = null;
        }, 120);
    }, [confirmingDelete]);

    React.useEffect(() => () => {
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
        }
    }, []);

    const showInlineActions = !swipeEnabled && (hovered || confirmingDelete);

    const itemContent = (
        <Pressable
            style={[
                styles.sessionItem,
                !swipeEnabled && showInlineActions && (confirmingDelete ? styles.sessionItemWithConfirmActions : styles.sessionItemWithHoverActions),
                selected && styles.sessionItemSelected,
                isSingle ? styles.sessionItemSingle :
                    isFirst ? styles.sessionItemFirst :
                        isLast ? styles.sessionItemLast : {}
            ]}
            onHoverIn={!swipeEnabled ? showHover : undefined}
            onHoverOut={!swipeEnabled ? scheduleHideHover : undefined}
            onPressIn={() => {
                if (confirmingDelete) {
                    setConfirmingDelete(false);
                    return;
                }
                if (isTablet) {
                    navigateToSession(session.id);
                }
            }}
            onPress={() => {
                if (confirmingDelete) {
                    setConfirmingDelete(false);
                    return;
                }
                if (!isTablet) {
                    navigateToSession(session.id);
                }
            }}
        >
            <View style={styles.avatarContainer}>
                <Avatar id={avatarId} size={48} monochrome={!sessionStatus.isConnected} flavor={inferSessionVendor(session)} />
                {session.draft && (
                    <View style={styles.draftIconContainer}>
                        <Ionicons
                            name="create-outline"
                            size={12}
                            style={styles.draftIconOverlay}
                        />
                    </View>
                )}
            </View>
            <View style={styles.sessionContent}>
                {/* Title line */}
                <View style={styles.sessionTitleRow}>
                    <Text style={[
                        styles.sessionTitle,
                        sessionStatus.isConnected ? styles.sessionTitleConnected : styles.sessionTitleDisconnected
                    ]} numberOfLines={1}> {/* {variant !== 'no-path' ? 1 : 2} - issue is we don't have anything to take this space yet and it looks strange - if summaries were more reliably generated, we can add this. While no summary - add something like "New session" or "Empty session", and extend summary to 2 lines once we have it */}
                        {sessionName}
                    </Text>
                    <VendorBadge vendor={inferSessionVendor(session)} />
                </View>

                {/* Subtitle line */}
                <Text style={styles.sessionSubtitle} numberOfLines={1}>
                    {sessionSubtitle}
                </Text>

                {/* Status line with dot */}
                <View style={styles.statusRow}>
                    <View style={styles.statusDotContainer}>
                        <StatusDot color={sessionStatus.statusDotColor} isPulsing={sessionStatus.isPulsing} />
                    </View>
                    <Text style={[
                        styles.statusText,
                        { color: sessionStatus.statusColor }
                    ]}>
                        {sessionStatus.statusText}
                    </Text>
                </View>
            </View>
            {!swipeEnabled && showInlineActions && (
                <View pointerEvents="box-none" style={styles.inlineActionsRail}>
                    {confirmingDelete ? (
                        <>
                            <Pressable
                                style={styles.inlineConfirmButton}
                                onHoverIn={showHover}
                                onHoverOut={scheduleHideHover}
                                onPress={(e) => {
                                    e.stopPropagation();
                                    setConfirmingDelete(false);
                                }}
                            >
                                <Text style={styles.inlineConfirmText}>
                                    {t('common.cancel')}
                                </Text>
                            </Pressable>
                            <Pressable
                                style={[styles.inlineConfirmButton, styles.inlineConfirmDeleteButton]}
                                onHoverIn={showHover}
                                onHoverOut={scheduleHideHover}
                                onPress={(e) => {
                                    e.stopPropagation();
                                    setConfirmingDelete(false);
                                    performDelete();
                                }}
                                disabled={deletingSession}
                            >
                                {deletingSession ? (
                                    <Ionicons name="hourglass-outline" size={14} color="#FFFFFF" />
                                ) : (
                                    <Text style={[styles.inlineConfirmText, styles.inlineConfirmDeleteText]}>
                                        {t('common.delete')}
                                    </Text>
                                )}
                            </Pressable>
                        </>
                    ) : (
                        <Pressable
                            style={({ pressed }) => [
                                styles.inlineActionButton,
                                pressed && styles.inlineActionButtonPressed
                            ]}
                            onHoverIn={showHover}
                            onHoverOut={scheduleHideHover}
                            onPressIn={(e) => e.stopPropagation()}
                            onPress={(e) => {
                                e.stopPropagation();
                                setConfirmingDelete(true);
                            }}
                            disabled={deletingSession}
                        >
                            <Ionicons name="trash-outline" size={16} color="#8E8E93" />
                        </Pressable>
                    )}
                </View>
            )}
        </Pressable>
    );

    const containerStyles = [
        styles.sessionItemContainer,
        isSingle ? styles.sessionItemContainerSingle :
            isFirst ? styles.sessionItemContainerFirst :
                isLast ? styles.sessionItemContainerLast : {}
    ];

    if (!swipeEnabled) {
        return (
            <View style={containerStyles}>
                {itemContent}
            </View>
        );
    }

    const renderRightActions = () => (
        <Pressable
            style={styles.swipeAction}
            onPress={handleDelete}
            disabled={deletingSession}
        >
            <Ionicons name="trash-outline" size={20} color="#FFFFFF" />
            <Text style={styles.swipeActionText} numberOfLines={2}>
                {t('sessionInfo.deleteSession')}
            </Text>
        </Pressable>
    );

    return (
        <View style={containerStyles}>
            <Swipeable
                ref={swipeableRef}
                renderRightActions={renderRightActions}
                overshootRight={false}
                enabled={!deletingSession}
            >
                {itemContent}
            </Swipeable>
        </View>
    );
});
