import React from 'react';
import { View, Pressable, Platform, ActivityIndicator } from 'react-native';
import { Swipeable } from 'react-native-gesture-handler';
import { Text } from '@/components/StyledText';
import { router, useRouter } from 'expo-router';
import { Session, Machine } from '@/sync/storageTypes';
import { Ionicons } from '@expo/vector-icons';
import { getSessionName, useSessionStatus, getSessionAvatarId, formatPathRelativeToHome } from '@/utils/sessionUtils';
import { Avatar } from './Avatar';
import { Typography } from '@/constants/Typography';
import { StatusDot } from './StatusDot';
import { useAllMachines, useSetting } from '@/sync/storage';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { isMachineOnline } from '@/utils/machineUtils';
import { machineSpawnNewSession, sessionDelete, sessionKill } from '@/sync/ops';
import { resolveAbsolutePath } from '@/utils/pathUtils';
import { storage } from '@/sync/storage';
import { Modal } from '@/modal';
import { t } from '@/text';
import { useNavigateToSession } from '@/hooks/useNavigateToSession';
import { useIsTablet } from '@/utils/responsive';
import { ProjectGitStatus } from './ProjectGitStatus';
import { useHappyAction } from '@/hooks/useHappyAction';
import { HappyError } from '@/utils/errors';
import { inferSessionVendor } from '@/hooks/useCapability';

const stylesheet = StyleSheet.create((theme, runtime) => ({
    container: {
        backgroundColor: theme.colors.groupped.background,
        paddingTop: 8,
    },
    projectCard: {
        backgroundColor: theme.colors.surface,
        marginBottom: 8,
        marginHorizontal: Platform.select({ ios: 16, default: 12 }),
        borderRadius: Platform.select({ ios: 10, default: 16 }),
        overflow: 'hidden',
        shadowColor: theme.colors.shadow.color,
        shadowOffset: { width: 0, height: 0.33 },
        shadowOpacity: theme.colors.shadow.opacity,
        shadowRadius: 0,
        elevation: 1,
    },
    sectionHeader: {
        paddingTop: 12,
        paddingBottom: Platform.select({ ios: 6, default: 8 }),
        paddingHorizontal: Platform.select({ ios: 32, default: 24 }),
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
    },
    sectionHeaderLeft: {
        flexDirection: 'row',
        alignItems: 'center',
        flex: 1,
        marginRight: 8,
    },
    sectionHeaderAvatar: {
        marginRight: 8,
    },
    sectionHeaderPath: {
        ...Typography.default('regular'),
        color: theme.colors.groupped.sectionTitle,
        fontSize: Platform.select({ ios: 13, default: 14 }),
        lineHeight: Platform.select({ ios: 18, default: 20 }),
        letterSpacing: Platform.select({ ios: -0.08, default: 0.1 }),
        fontWeight: Platform.select({ ios: 'normal', default: '500' }),
        flex: 1,
    },
    sessionRow: {
        height: 56,
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: 16,
        backgroundColor: theme.colors.surface,
    },
    sessionRowWithHoverActions: {
        paddingRight: 104,
    },
    sessionRowWithConfirmActions: {
        paddingRight: 168,
    },
    sessionRowWithBorder: {
        borderBottomWidth: StyleSheet.hairlineWidth,
        borderBottomColor: theme.colors.divider,
    },
    sessionRowSelected: {
        backgroundColor: theme.colors.surfaceSelected,
    },
    sessionContent: {
        flex: 1,
        justifyContent: 'center',
    },
    sessionTitleRow: {
        flexDirection: 'row',
        alignItems: 'center',
    },
    sessionTitle: {
        fontSize: 15,
        flex: 1,
        ...Typography.default('regular'),
    },
    sessionTitleConnected: {
        color: theme.colors.text,
    },
    sessionTitleDisconnected: {
        color: theme.colors.textSecondary,
    },
    statusDotContainer: {
        alignItems: 'center',
        justifyContent: 'center',
        width: 16,
        height: 16,
    },
    newSessionButton: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'flex-start',
        height: 56,
        paddingHorizontal: 16,
        borderTopWidth: StyleSheet.hairlineWidth,
        borderTopColor: theme.colors.divider,
        backgroundColor: theme.colors.surface,
    },
    newSessionButtonDisabled: {
        opacity: 0.4,
    },
    newSessionButtonContent: {
        flexDirection: 'row',
        alignItems: 'center',
    },
    newSessionButtonIcon: {
        marginRight: 8,
        width: 16,
        alignItems: 'center',
        justifyContent: 'center',
    },
    newSessionButtonText: {
        fontSize: 15,
        color: theme.colors.textSecondary,
        ...Typography.default('regular'),
    },
    swipeActionsContainer: {
        flexDirection: 'row',
        height: '100%',
    },
    swipeAction: {
        width: 112,
        height: '100%',
        alignItems: 'center',
        justifyContent: 'center',
    },
    swipeActionArchive: {
        backgroundColor: '#FF9500',
    },
    swipeActionDelete: {
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
        height: 28,
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
}));

interface ActiveSessionsGroupProps {
    sessions: Session[];
    selectedSessionId?: string;
}


export function ActiveSessionsGroupCompact({ sessions, selectedSessionId }: ActiveSessionsGroupProps) {
    const styles = stylesheet;
    const machines = useAllMachines();

    const machinesMap = React.useMemo(() => {
        const map: Record<string, Machine> = {};
        machines.forEach(machine => {
            map[machine.id] = machine;
        });
        return map;
    }, [machines]);

    // Group sessions by project, then associate with machine
    const projectGroups = React.useMemo(() => {
        const groups = new Map<string, {
            path: string;
            displayPath: string;
            machines: Map<string, {
                machine: Machine | null;
                machineName: string;
                sessions: Session[];
            }>;
        }>();

        sessions.forEach(session => {
            const projectPath = session.metadata?.path || '';
            const unknownText = t('status.unknown');
            const machineId = session.metadata?.machineId || unknownText;

            // Get machine info
            const machine = machineId !== unknownText ? machinesMap[machineId] : null;
            const machineName = machine?.decryptionFailed
                ? '🔐 需要导入设备密钥'
                : (machine?.metadata?.displayName ||
                   machine?.metadata?.host ||
                   (machineId !== unknownText ? machineId : `<${unknownText}>`));

            // Get or create project group
            let projectGroup = groups.get(projectPath);
            if (!projectGroup) {
                const displayPath = formatPathRelativeToHome(projectPath, session.metadata?.homeDir);
                projectGroup = {
                    path: projectPath,
                    displayPath,
                    machines: new Map()
                };
                groups.set(projectPath, projectGroup);
            }

            // Get or create machine group within project
            let machineGroup = projectGroup.machines.get(machineId);
            if (!machineGroup) {
                machineGroup = {
                    machine,
                    machineName,
                    sessions: []
                };
                projectGroup.machines.set(machineId, machineGroup);
            }

            // Add session to machine group
            machineGroup.sessions.push(session);
        });

        // Sort sessions within each machine group by creation time (newest first)
        groups.forEach(projectGroup => {
            projectGroup.machines.forEach(machineGroup => {
                machineGroup.sessions.sort((a, b) => b.createdAt - a.createdAt);
            });
        });

        return groups;
    }, [sessions, machinesMap]);

    // Sort project groups by display path
    const sortedProjectGroups = React.useMemo(() => {
        return Array.from(projectGroups.entries()).sort(([, groupA], [, groupB]) => {
            return groupA.displayPath.localeCompare(groupB.displayPath);
        });
    }, [projectGroups]);

    return (
        <View style={styles.container}>
            {sortedProjectGroups.map(([projectPath, projectGroup]) => {

                // Get the avatar ID from the first session
                const firstSession = Array.from(projectGroup.machines.values())[0]?.sessions[0];
                const avatarId = firstSession ? getSessionAvatarId(firstSession) : undefined;

                return (
                    <View key={projectPath}>
                        {/* Section header on grouped background */}
                        <View style={styles.sectionHeader}>
                            <View style={styles.sectionHeaderLeft}>
                                {avatarId && (
                                    <View style={styles.sectionHeaderAvatar}>
                                        <Avatar id={avatarId} size={24} flavor={inferSessionVendor(firstSession)} />
                                    </View>
                                )}
                                <Text style={styles.sectionHeaderPath}>
                                    {projectGroup.displayPath}
                                </Text>
                            </View>
                            {/* Show git status instead of machine name */}
                            {firstSession ? (
                                <ProjectGitStatus sessionId={firstSession.id} />
                            ) : null}
                        </View>

                        {/* Card with just the sessions */}
                        <View style={styles.projectCard}>
                            {/* Sessions grouped by machine within the card */}
                            {Array.from(projectGroup.machines.entries())
                                .sort(([, machineA], [, machineB]) => machineA.machineName.localeCompare(machineB.machineName))
                                .map(([machineId, machineGroup]) => (
                                    <View key={`${projectPath}-${machineId}`}>
                                        {machineGroup.sessions.map((session, index) => (
                                            <CompactSessionRow
                                                key={session.id}
                                                session={session}
                                                selected={selectedSessionId === session.id}
                                                showBorder={index < machineGroup.sessions.length - 1 ||
                                                    Array.from(projectGroup.machines.keys()).indexOf(machineId) < projectGroup.machines.size - 1}
                                            />
                                        ))}
                                    </View>
                                ))}
                        </View>
                    </View>
                );
            })}
        </View>
    );
}

// Compact session row component with status line
const CompactSessionRow = React.memo(({ session, selected, showBorder }: { session: Session; selected?: boolean; showBorder?: boolean }) => {
    const styles = stylesheet;
    const { theme } = useUnistyles();
    const sessionStatus = useSessionStatus(session);
    const sessionName = getSessionName(session);
    const navigateToSession = useNavigateToSession();
    const isTablet = useIsTablet();
    const swipeableRef = React.useRef<Swipeable | null>(null);
    const swipeEnabled = Platform.OS !== 'web';
    const [hovered, setHovered] = React.useState(false);
    const [confirmingDelete, setConfirmingDelete] = React.useState(false);
    const hideHoverTimeoutRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

    const [archivingSession, performArchive] = useHappyAction(async () => {
        const result = await sessionKill(session.id);
        if (!result.success) {
            throw new HappyError(result.message || t('sessionInfo.failedToArchiveSession'), false);
        }
    });
    const [deletingSession, performDelete] = useHappyAction(async () => {
        const result = await sessionDelete(session.id);
        if (!result.success) {
            throw new HappyError(result.message || t('sessionInfo.failedToDeleteSession'), false);
        }
    });

    const handleArchive = React.useCallback(() => {
        swipeableRef.current?.close();
        Modal.alert(
            t('sessionInfo.archiveSession'),
            t('sessionInfo.archiveSessionConfirm'),
            [
                { text: t('common.cancel'), style: 'cancel' },
                {
                    text: t('sessionInfo.archiveSession'),
                    style: 'destructive',
                    onPress: performArchive
                }
            ]
        );
    }, [performArchive]);
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
                styles.sessionRow,
                !swipeEnabled && showInlineActions && (confirmingDelete ? styles.sessionRowWithConfirmActions : styles.sessionRowWithHoverActions),
                showBorder && styles.sessionRowWithBorder,
                selected && styles.sessionRowSelected
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
            <View style={styles.sessionContent}>
                {/* Title line with status */}
                <View style={styles.sessionTitleRow}>
                    {/* Status dot or draft icon on the left */}
                    {(() => {
                        // Show draft icon when online with draft
                        if (sessionStatus.state === 'waiting' && session.draft) {
                            return (
                                <Ionicons
                                    name="create-outline"
                                    size={14}
                                    color={theme.colors.textSecondary}
                                    style={{ marginRight: 8 }}
                                />
                            );
                        }
                        
                        // Show status dot only for permission_required/thinking states
                        if (sessionStatus.state === 'permission_required' || sessionStatus.state === 'thinking') {
                            return (
                                <View style={[styles.statusDotContainer, { marginRight: 8 }]}>
                                    <StatusDot 
                                        color={sessionStatus.statusDotColor} 
                                        isPulsing={sessionStatus.isPulsing} 
                                    />
                                </View>
                            );
                        }
                        
                        // Show grey dot for online without draft
                        if (sessionStatus.state === 'waiting') {
                            return (
                                <View style={[styles.statusDotContainer, { marginRight: 8 }]}>
                                    <StatusDot 
                                        color={theme.colors.textSecondary} 
                                        isPulsing={false} 
                                    />
                                </View>
                            );
                        }
                        
                        return null;
                    })()}
                    
                    <Text
                        style={[
                            styles.sessionTitle,
                            sessionStatus.isConnected ? styles.sessionTitleConnected : styles.sessionTitleDisconnected
                        ]}
                        numberOfLines={2}
                    >
                        {sessionName}
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
                                disabled={archivingSession || deletingSession}
                            >
                                {deletingSession ? (
                                    <ActivityIndicator size="small" color="#FFFFFF" />
                                ) : (
                                    <Text style={[styles.inlineConfirmText, styles.inlineConfirmDeleteText]}>
                                        {t('common.delete')}
                                    </Text>
                                )}
                            </Pressable>
                        </>
                    ) : (
                        <>
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
                                    handleArchive();
                                }}
                                disabled={archivingSession || deletingSession}
                            >
                                <Ionicons name="archive-outline" size={16} color={theme.colors.textSecondary} />
                            </Pressable>
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
                                disabled={archivingSession || deletingSession}
                            >
                                <Ionicons name="trash-outline" size={16} color={theme.colors.textSecondary} />
                            </Pressable>
                        </>
                    )}
                </View>
            )}
        </Pressable>
    );

    if (!swipeEnabled) {
        return itemContent;
    }

    const renderRightActions = () => (
        <View style={styles.swipeActionsContainer}>
            <Pressable
                style={[styles.swipeAction, styles.swipeActionArchive]}
                onPress={handleArchive}
                disabled={archivingSession || deletingSession}
            >
                <Ionicons name="archive-outline" size={20} color="#FFFFFF" />
                <Text style={styles.swipeActionText} numberOfLines={2}>
                    {t('sessionInfo.archiveSession')}
                </Text>
            </Pressable>
            <Pressable
                style={[styles.swipeAction, styles.swipeActionDelete]}
                onPress={handleDelete}
                disabled={archivingSession || deletingSession}
            >
                <Ionicons name="trash-outline" size={20} color="#FFFFFF" />
                <Text style={styles.swipeActionText} numberOfLines={2}>
                    {t('sessionInfo.deleteSession')}
                </Text>
            </Pressable>
        </View>
    );

    return (
        <Swipeable
            ref={swipeableRef}
            renderRightActions={renderRightActions}
            overshootRight={false}
            enabled={!archivingSession && !deletingSession}
        >
            {itemContent}
        </Swipeable>
    );
});
