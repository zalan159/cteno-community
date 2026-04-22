import * as React from 'react';
import { useState, useMemo, useCallback, useEffect, useRef } from 'react';
import { View, Pressable, ActivityIndicator, Platform } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { usePathname, useRouter } from 'expo-router';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { ItemList } from '@/components/ItemList';
import { PersonaCard } from '@/components/PersonaCard';
import { CreateWorkspaceModal } from '@/components/CreateWorkspaceModal';
import { NewProjectModal } from '@/components/NewProjectModal';
import { AvatarPickerModal } from '@/components/AvatarPickerModal';
import { usePersonas } from '@/hooks/usePersonas';
import { useAgentWorkspaces } from '@/hooks/useAgentWorkspaces';
import { usePersonaUnread } from '@/hooks/usePersonaUnread';
import { useAllMachines, useAllSessions, useLocalSettingMutable, useSession } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { useIsTablet } from '@/utils/responsive';
import { sync } from '@/sync/sync';
import { machineBootstrapWorkspace, machineDeleteAgentWorkspace, machineListModels } from '@/sync/ops';
import { layout } from '@/components/layout';
import { UpdateBanner } from '@/components/UpdateBanner';
import { t } from '@/text';
import type { Persona } from '@/sync/storageTypes';
import { useScheduledTasks } from '@/hooks/useScheduledTasks';
import { frontendLog } from '@/utils/tauri';
import type { WorkspaceRoleVendorOverrides, WorkspaceTemplateId, VendorName } from '@/sync/ops';
import { Modal } from '@/modal';
import { VendorSelector } from '@/components/VendorSelector';

/** Extract the last directory component from a path */
function dirName(path: string): string {
    if (!path) return '';
    const trimmed = path.replace(/\/+$/, '');
    const lastSlash = trimmed.lastIndexOf('/');
    return lastSlash >= 0 ? trimmed.slice(lastSlash + 1) : trimmed;
}

/** Group key for a persona — uses workdir or empty string for unlinked */
function groupKey(persona: Persona): string {
    return persona.workdir?.trim() || '';
}

function workspaceTemplateLabel(templateId: string | null | undefined): string | null {
    switch (templateId) {
        case 'group-chat':
        case 'coding-studio':
            return '工作间 · 群聊';
        case 'gated-tasks':
        case 'task-gate-coding':
        case 'task-gate-coding-manual':
            return '工作间 · 门控任务';
        case 'autoresearch':
            return '工作间 · 自主研究';
        default:
            return templateId ? `工作间 · ${templateId}` : null;
    }
}

interface ProjectGroup {
    key: string;       // workdir path or '' for unlinked
    label: string;     // display name (last dir component)
    personas: Persona[];
    hasOnline: boolean; // whether any persona in this group is online
}

const PersonaSubsectionHeader = React.memo(({
    label,
}: {
    label: string;
}) => {
    const { theme } = useUnistyles();

    return (
        <View style={{ paddingHorizontal: 16, paddingTop: 10, paddingBottom: 6 }}>
            <Text style={{
                fontSize: 11,
                color: theme.colors.textSecondary,
                textTransform: 'uppercase',
                letterSpacing: 0.6,
                ...Typography.default('semiBold'),
            }}>
                {label}
            </Text>
        </View>
    );
});

/** Wrapper that calls hooks per-persona (hooks can't be called inside map). */
function PersonaListItem({ persona, onPress, onDelete, onAvatarPress, isSelected, hasScheduledTask, workspaceLabel, deleting }: {
    persona: Persona;
    onPress: () => void;
    onDelete: () => void | Promise<void>;
    onAvatarPress: () => void;
    isSelected?: boolean;
    hasScheduledTask?: boolean;
    workspaceLabel?: string | null;
    deleting?: boolean;
}) {
    const { unreadCount, lastMessage } = usePersonaUnread(persona.chatSessionId);
    const session = useSession(persona.chatSessionId);
    const isOffline = !session || session.presence !== 'online';

    // Preload messages for this persona's session
    React.useEffect(() => {
        sync.onSessionVisible(persona.chatSessionId);
    }, [persona.chatSessionId]);

    return (
        <PersonaCard
            persona={persona}
            onPress={onPress}
            onDelete={onDelete}
            onAvatarPress={onAvatarPress}
            lastMessage={lastMessage}
            unreadCount={unreadCount}
            isOffline={isOffline}
            isSelected={isSelected}
            hasScheduledTask={hasScheduledTask}
            isThinking={session?.thinking === true}
            sessionProfileId={session?.metadata?.modelId}
            workspaceLabel={workspaceLabel}
            deleting={deleting}
        />
    );
}

/** Collapsible project section header */
const ProjectSectionHeader = React.memo(({
    label,
    collapsed,
    onToggle,
    isUnlinked,
    onAddChat,
    onDeleteProject,
    deletingProject,
}: {
    label: string;
    collapsed: boolean;
    onToggle: () => void;
    isUnlinked: boolean;
    onAddChat?: () => void;
    onDeleteProject?: () => void | Promise<void>;
    deletingProject?: boolean;
}) => {
    const { theme } = useUnistyles();
    const isWeb = Platform.OS === 'web';
    const [hovered, setHovered] = React.useState(false);
    const [confirmingDelete, setConfirmingDelete] = React.useState(false);
    const hideHoverTimeoutRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

    const showDeleteControls = !!onDeleteProject && (!isWeb || hovered || confirmingDelete);

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

    const handleConfirmDelete = React.useCallback(async () => {
        if (!onDeleteProject || deletingProject) return;
        try {
            await onDeleteProject();
        } finally {
            setConfirmingDelete(false);
        }
    }, [deletingProject, onDeleteProject]);

    return (
        <Pressable
            onPress={() => {
                if (confirmingDelete) {
                    setConfirmingDelete(false);
                    return;
                }
                onToggle();
            }}
            onHoverIn={isWeb ? showHover : undefined}
            onHoverOut={isWeb ? scheduleHideHover : undefined}
            style={({ pressed }) => ({
                flexDirection: 'row',
                alignItems: 'center',
                paddingHorizontal: 16,
                height: 40,
                width: '100%',
                alignSelf: 'stretch',
                backgroundColor: pressed
                    ? theme.colors.surfacePressed
                    : theme.colors.groupped.background,
            })}
        >
            <Ionicons
                name={collapsed ? 'chevron-forward' : 'chevron-down'}
                size={14}
                color={theme.colors.textSecondary}
                style={{ marginRight: 6 }}
            />
            <Ionicons
                name={isUnlinked ? 'cube-outline' : 'folder-outline'}
                size={16}
                color={theme.colors.textSecondary}
                style={{ marginRight: 8 }}
            />
            <Text style={{
                fontSize: 16,
                color: theme.colors.text,
                flex: 1,
                ...Typography.default('semiBold'),
            }} numberOfLines={1}>
                {label}
            </Text>
            {showDeleteControls && (
                confirmingDelete ? (
                    <Pressable
                        accessible={false}
                        style={{ flexDirection: 'row', alignItems: 'center', marginLeft: 8, gap: 8 }}
                        onHoverIn={isWeb ? showHover : undefined}
                        onHoverOut={isWeb ? scheduleHideHover : undefined}
                    >
                        <Pressable
                            onPress={(e) => {
                                e.stopPropagation?.();
                                setConfirmingDelete(false);
                            }}
                            onHoverIn={isWeb ? showHover : undefined}
                            onHoverOut={isWeb ? scheduleHideHover : undefined}
                            hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
                            style={({ pressed }) => ({
                                paddingHorizontal: 10,
                                paddingVertical: 5,
                                borderRadius: 999,
                                backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surfaceHigh,
                            })}
                        >
                            <Text style={{
                                fontSize: 12,
                                color: theme.colors.textSecondary,
                                ...Typography.default('semiBold'),
                            }}>
                                {t('common.cancel')}
                            </Text>
                        </Pressable>
                        <Pressable
                            onPress={(e) => {
                                e.stopPropagation?.();
                                void handleConfirmDelete();
                            }}
                            onHoverIn={isWeb ? showHover : undefined}
                            onHoverOut={isWeb ? scheduleHideHover : undefined}
                            disabled={deletingProject}
                            hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
                            style={({ pressed }) => ({
                                minWidth: 72,
                                paddingHorizontal: 12,
                                paddingVertical: 5,
                                borderRadius: 999,
                                alignItems: 'center',
                                justifyContent: 'center',
                                backgroundColor: deletingProject
                                    ? theme.colors.surfacePressed
                                    : (pressed ? theme.colors.status.error : theme.colors.deleteAction),
                            })}
                        >
                            {deletingProject ? (
                                <ActivityIndicator size={12} color="#FFFFFF" />
                            ) : (
                                <Text style={{
                                    fontSize: 12,
                                    color: '#FFFFFF',
                                    ...Typography.default('semiBold'),
                                }}>
                                    {t('common.delete')}
                                </Text>
                            )}
                        </Pressable>
                    </Pressable>
                ) : (
                    <Pressable
                        onPress={(e) => {
                            e.stopPropagation?.();
                            setConfirmingDelete(true);
                        }}
                        onHoverIn={isWeb ? showHover : undefined}
                        onHoverOut={isWeb ? scheduleHideHover : undefined}
                        disabled={deletingProject}
                        hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
                        style={({ pressed }) => ({
                            marginLeft: 8,
                            width: 28,
                            height: 28,
                            borderRadius: 14,
                            alignItems: 'center',
                            justifyContent: 'center',
                            opacity: deletingProject ? 0.5 : 1,
                            backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                        })}
                    >
                        <Ionicons name="trash-outline" size={16} color={theme.colors.deleteAction} />
                    </Pressable>
                )
            )}
            {onAddChat && (
                <Pressable
                    onPress={(e) => {
                        e.stopPropagation?.();
                        onAddChat();
                    }}
                    hitSlop={{ top: 8, bottom: 8, left: 8, right: 8 }}
                    style={({ pressed }) => ({
                        marginLeft: 8,
                        opacity: pressed ? 0.5 : 1,
                    })}
                >
                    <Ionicons name="add" size={18} color={theme.colors.textSecondary} />
                </Pressable>
            )}
        </Pressable>
    );
});

export const PersonaView = React.memo(() => {
    const { theme } = useUnistyles();
    const router = useRouter();
    const pathname = usePathname();
    const isTablet = useIsTablet();
    const machines = useAllMachines();
    const sessions = useAllSessions();
    const [showNewProject, setShowNewProject] = useState(false);
    const [showCreateWorkspace, setShowCreateWorkspace] = useState(false);
    const [workspaceWorkdir, setWorkspaceWorkdir] = useState<string>('~/');
    const [isCreating, setIsCreating] = useState(false);
    const [deletingProjectKeys, setDeletingProjectKeys] = useState<Set<string>>(new Set());
    const [deletingPersonaIds, setDeletingPersonaIds] = useState<Set<string>>(new Set());
    const [selectedMachineIdFilter] = useLocalSettingMutable('selectedMachineIdFilter');
    const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(new Set());

    // Detect selected persona ID from route (for tablet split-view highlighting)
    const selectedPersonaId = useMemo(() => {
        if (!isTablet) return undefined;
        if (pathname.startsWith('/persona/')) {
            const parts = pathname.split('/');
            return parts[2] || undefined;
        }
        return undefined;
    }, [isTablet, pathname]);

    // Use selected machine filter, fall back to auto-selection
    const machineId = useMemo(() => {
        // If a specific machine is selected in the filter, use it
        if (selectedMachineIdFilter) {
            return selectedMachineIdFilter;
        }
        // Fall back: try machines with sessions first
        const machineIds = new Set<string>();
        for (const session of sessions) {
            const mid = session.metadata?.machineId;
            if (mid) machineIds.add(mid);
        }
        for (const mid of machineIds) {
            if (machines.some(m => m.id === mid && isMachineOnline(m))) {
                return mid;
            }
        }
        const online = machines.find(m => isMachineOnline(m));
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : undefined;
    }, [selectedMachineIdFilter, machines, sessions]);
    const selectedMachine = useMemo(
        () => machines.find((machine) => machine.id === machineId),
        [machineId, machines]
    );

    const { personas: rawPersonas, loading, createPersona, deletePersona, updatePersona, refresh } = usePersonas({
        machineId,
    });
    const { workspaces, refresh: refreshWorkspaces } = useAgentWorkspaces({ machineId });

    // Log when persona list first renders with data
    const personaRenderedRef = useRef(false);
    useEffect(() => {
        if (rawPersonas.length > 0 && !personaRenderedRef.current) {
            personaRenderedRef.current = true;
            frontendLog(`🎨 PersonaView: first render with ${rawPersonas.length} personas`);
        }
    }, [rawPersonas]);

    const { tasks: scheduledTasks } = useScheduledTasks({ machineId, pollingInterval: 60000 });
    const personaIdsWithSchedule = useMemo(() => {
        const ids = new Set<string>();
        for (const task of scheduledTasks) {
            if (task.persona_id && task.enabled) ids.add(task.persona_id);
        }
        return ids;
    }, [scheduledTasks]);

    const workspaceMap = useMemo(() => {
        return new Map(workspaces.map((workspace) => [workspace.persona.id, workspace]));
    }, [workspaces]);

    // Sort personas (online first), then group by workdir
    const projectGroups = useMemo(() => {
        const sessionMap = new Map(sessions.map(s => [s.id, s]));
        const sorted = [...rawPersonas].sort((a, b) => {
            const aOnline = sessionMap.get(a.chatSessionId)?.presence === 'online' ? 0 : 1;
            const bOnline = sessionMap.get(b.chatSessionId)?.presence === 'online' ? 0 : 1;
            return aOnline - bOnline;
        });

        // Group by workdir
        const groupMap = new Map<string, Persona[]>();
        for (const p of sorted) {
            const key = groupKey(p);
            const list = groupMap.get(key);
            if (list) {
                list.push(p);
            } else {
                groupMap.set(key, [p]);
            }
        }

        // Build ProjectGroup array
        const groups: ProjectGroup[] = [];
        for (const [key, personas] of groupMap) {
            const hasOnline = personas.some(
                p => sessionMap.get(p.chatSessionId)?.presence === 'online'
            );
            groups.push({
                key,
                label: key ? dirName(key) : t('persona.unlinkedProject'),
                personas,
                hasOnline,
            });
        }

        // Sort groups: has online first, then alphabetical
        groups.sort((a, b) => {
            if (a.hasOnline !== b.hasOnline) return a.hasOnline ? -1 : 1;
            // Unlinked always last
            if (!a.key) return 1;
            if (!b.key) return -1;
            return a.label.localeCompare(b.label);
        });

        return groups;
    }, [rawPersonas, sessions]);

    const toggleGroup = useCallback((key: string) => {
        setCollapsedGroups(prev => {
            const next = new Set(prev);
            if (next.has(key)) {
                next.delete(key);
            } else {
                next.add(key);
            }
            return next;
        });
    }, []);

    // Avatar picker state
    const [avatarPickerPersona, setAvatarPickerPersona] = useState<Persona | null>(null);

    const handleCreate = useCallback(async (params: {
        name: string;
        description: string;
        modelId?: string;
        avatarId: string;
        workdir: string;
    }) => {
        const persona = await createPersona(params);
        if (persona?.id) {
            router.push(`/persona/${persona.id}` as any);
        }
    }, [createPersona, router]);

    const handleQuickCreate = useCallback(async (workdir: string, agent?: VendorName) => {
        if (isCreating) return;
        setIsCreating(true);
        try {
            let modelId: string | undefined;
            if (machineId && agent && agent !== 'cteno') {
                try {
                    const result = await machineListModels(machineId, agent);
                    modelId = result.defaultModelId || undefined;
                } catch (error) {
                    console.warn('[PersonaView] Failed to prefetch vendor default model:', error);
                }
            }
            const persona = await createPersona({ workdir, agent, modelId });
            if (persona?.id) {
                // Refresh sessions so the persona's chatSessionId is in storage
                // before we navigate to the persona detail page
                await sync.refreshSessions();
                router.push(`/persona/${persona.id}` as any);
            }
        } finally {
            setIsCreating(false);
        }
    }, [createPersona, router, isCreating, machineId]);

    const handleNewTaskWithVendor = useCallback((workdir: string = '~/') => {
        let modalId: string;
        modalId = Modal.show({
            component: VendorSelector as any,
            props: {
                value: null,
                onChange: (vendor: VendorName) => {
                    Modal.hide(modalId);
                    handleQuickCreate(workdir, vendor);
                },
                title: t('newSession.selectAgent'),
                machineId,
                onCreateWorkspace: () => {
                    Modal.hide(modalId);
                    setWorkspaceWorkdir(workdir);
                    setShowCreateWorkspace(true);
                },
            },
        });
    }, [handleQuickCreate, machineId]);

    const handleCreateWorkspace = useCallback(async (params: {
        templateId: WorkspaceTemplateId;
        name: string;
        workdir: string;
        roleVendorOverrides: WorkspaceRoleVendorOverrides;
    }) => {
        if (!machineId) return;
        const result = await machineBootstrapWorkspace(machineId, params);
        if (!result.success || !result.workspace?.personaId) {
            throw new Error(result.error || 'Failed to create workspace');
        }
        await Promise.all([refresh(), refreshWorkspaces(), sync.refreshSessions()]);
        router.push(`/persona/${result.workspace.personaId}` as any);
    }, [machineId, refresh, refreshWorkspaces, router]);

    const handleDelete = useCallback(async (id: string) => {
        if (deletingPersonaIds.has(id)) return;
        const workspace = workspaceMap.get(id);

        setDeletingPersonaIds((prev) => {
            const next = new Set(prev);
            next.add(id);
            return next;
        });

        try {
            if (workspace) {
                if (!machineId) {
                    throw new Error('No machine selected for workspace deletion');
                }
                const result = await machineDeleteAgentWorkspace(machineId, id);
                if (!result.success) {
                    throw new Error(result.error || 'Failed to delete workspace');
                }
            } else {
                await deletePersona(id);
            }
            await refreshWorkspaces();
            await refresh();
        } catch (err) {
            console.error(`Failed to delete ${workspace ? 'workspace' : 'persona'}:`, err);
        } finally {
            setDeletingPersonaIds((prev) => {
                const next = new Set(prev);
                next.delete(id);
                return next;
            });
        }
    }, [deletePersona, deletingPersonaIds, machineId, refresh, refreshWorkspaces, workspaceMap]);

    const handleDeleteProject = useCallback(async (group: ProjectGroup) => {
        if (!group.key || group.personas.length === 0) return;

        setDeletingProjectKeys((prev) => {
            const next = new Set(prev);
            next.add(group.key);
            return next;
        });

        let failedCount = 0;
        try {
            for (const persona of group.personas) {
                try {
                    const workspace = workspaceMap.get(persona.id);
                    if (workspace) {
                        if (!machineId) {
                            throw new Error('No machine selected for workspace deletion');
                        }
                        const result = await machineDeleteAgentWorkspace(machineId, persona.id);
                        if (!result.success) {
                            throw new Error(result.error || 'Failed to delete workspace');
                        }
                    } else {
                        await deletePersona(persona.id);
                    }
                } catch (err) {
                    failedCount += 1;
                    console.error(`Failed to delete persona ${persona.id} during project delete:`, err);
                }
            }

            await Promise.all([refresh(), refreshWorkspaces()]);

            if (failedCount > 0) {
                console.error(t('persona.deleteProjectFailed', { count: failedCount }));
            }
        } finally {
            setDeletingProjectKeys((prev) => {
                const next = new Set(prev);
                next.delete(group.key);
                return next;
            });
        }
    }, [deletePersona, machineId, refresh, refreshWorkspaces, workspaceMap]);

    const handlePersonaPress = useCallback((personaId: string) => {
        router.push(`/persona/${personaId}` as any);
    }, [router]);

    const renderPersonaSection = useCallback((personas: Persona[]) => {
        const workspacePersonas = personas.filter((persona) => workspaceMap.has(persona.id));
        const regularPersonas = personas.filter((persona) => !workspaceMap.has(persona.id));

        return (
            <>
                {workspacePersonas.length > 0 && (
                    <>
                        <PersonaSubsectionHeader label="工作间" />
                        {workspacePersonas.map((persona) => (
                            <PersonaListItem
                                key={persona.id}
                                persona={persona}
                                onPress={() => handlePersonaPress(persona.id)}
                                onDelete={() => handleDelete(persona.id)}
                                onAvatarPress={() => setAvatarPickerPersona(persona)}
                                isSelected={persona.id === selectedPersonaId}
                                hasScheduledTask={personaIdsWithSchedule.has(persona.id)}
                                workspaceLabel={workspaceTemplateLabel(workspaceMap.get(persona.id)?.binding.templateId)}
                                deleting={deletingPersonaIds.has(persona.id)}
                            />
                        ))}
                    </>
                )}
                {regularPersonas.length > 0 && (
                    <>
                        {workspacePersonas.length > 0 && <PersonaSubsectionHeader label="聊天" />}
                        {regularPersonas.map((persona) => (
                            <PersonaListItem
                                key={persona.id}
                                persona={persona}
                                onPress={() => handlePersonaPress(persona.id)}
                                onDelete={() => handleDelete(persona.id)}
                                onAvatarPress={() => setAvatarPickerPersona(persona)}
                                isSelected={persona.id === selectedPersonaId}
                                hasScheduledTask={personaIdsWithSchedule.has(persona.id)}
                                workspaceLabel={workspaceTemplateLabel(workspaceMap.get(persona.id)?.binding.templateId)}
                                deleting={deletingPersonaIds.has(persona.id)}
                            />
                        ))}
                    </>
                )}
            </>
        );
    }, [
        handleDelete,
        handlePersonaPress,
        deletingPersonaIds,
        personaIdsWithSchedule,
        selectedPersonaId,
        workspaceMap,
    ]);

    if (!machineId) {
        return (
            <ItemList style={{ paddingTop: 0 }}>
                <View style={{
                    maxWidth: layout.maxWidth,
                    alignSelf: 'center',
                    width: '100%',
                    padding: 24,
                    alignItems: 'center',
                }}>
                    <Ionicons name="cloud-offline-outline" size={40} color={theme.colors.textSecondary} />
                    <Text style={{
                        marginTop: 12,
                        fontSize: 16,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        ...Typography.default(),
                    }}>
                        {t('persona.noMachine')}
                    </Text>
                </View>
            </ItemList>
        );
    }

    const allPersonas = projectGroups.flatMap(g => g.personas);

    return (
        <ItemList style={{ paddingTop: 0 }}>
            <UpdateBanner />

            {/* Action buttons */}
            <View style={{
                maxWidth: layout.maxWidth,
                alignSelf: 'center',
                width: '100%',
                paddingHorizontal: 16,
                paddingTop: 16,
                paddingBottom: 8,
                gap: 8,
            }}>
                {/* New Task */}
                <Pressable
                    onPress={() => handleNewTaskWithVendor('~/')}
                    disabled={isCreating}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        justifyContent: 'center',
                        paddingVertical: 10,
                        borderRadius: 10,
                        opacity: isCreating ? 0.6 : 1,
                        backgroundColor: pressed
                            ? theme.colors.surfacePressed
                            : theme.colors.surfaceHigh,
                    })}
                >
                    {isCreating ? (
                        <ActivityIndicator size={16} color={theme.colors.text} />
                    ) : (
                        <Ionicons name="chatbubble-outline" size={16} color={theme.colors.text} />
                    )}
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.text,
                        marginLeft: 6,
                        ...Typography.default('semiBold'),
                    }}>
                        {isCreating ? t('persona.creating') : t('persona.newChat')}
                    </Text>
                </Pressable>
                {/* Skills */}
                <Pressable
                    onPress={() => router.push('/skills' as any)}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        justifyContent: 'center',
                        paddingVertical: 10,
                        borderRadius: 10,
                        backgroundColor: pressed
                            ? theme.colors.surfacePressed
                            : theme.colors.surfaceHigh,
                    })}
                >
                    <Ionicons name="flash-outline" size={16} color={theme.colors.text} />
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.text,
                        marginLeft: 6,
                        ...Typography.default('semiBold'),
                    }}>
                        {t('skills.title')}
                    </Text>
                </Pressable>
                {/* MCP servers — per-machine config, scoped to the currently selected machine */}
                <Pressable
                    onPress={() => machineId && router.push(`/settings/mcp?machineId=${machineId}` as any)}
                    disabled={!machineId}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        justifyContent: 'center',
                        paddingVertical: 10,
                        borderRadius: 10,
                        opacity: machineId ? 1 : 0.4,
                        backgroundColor: pressed
                            ? theme.colors.surfacePressed
                            : theme.colors.surfaceHigh,
                    })}
                >
                    <Ionicons name="git-network-outline" size={16} color={theme.colors.text} />
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.text,
                        marginLeft: 6,
                        ...Typography.default('semiBold'),
                    }}>
                        {t('settings.mcp')}
                    </Text>
                </Pressable>
                {/* Scheduled tasks — per-machine */}
                <Pressable
                    onPress={() => machineId && router.push(`/settings/scheduled-tasks?machineId=${machineId}` as any)}
                    disabled={!machineId}
                    style={({ pressed }) => ({
                        flexDirection: 'row',
                        alignItems: 'center',
                        justifyContent: 'center',
                        paddingVertical: 10,
                        borderRadius: 10,
                        opacity: machineId ? 1 : 0.4,
                        backgroundColor: pressed
                            ? theme.colors.surfacePressed
                            : theme.colors.surfaceHigh,
                    })}
                >
                    <Ionicons name="timer-outline" size={16} color={theme.colors.text} />
                    <Text style={{
                        fontSize: 14,
                        color: theme.colors.text,
                        marginLeft: 6,
                        ...Typography.default('semiBold'),
                    }}>
                        {t('settingsScheduledTasks.title')}
                    </Text>
                </Pressable>
                {/* New Project row */}
                <View style={{ flexDirection: 'row', justifyContent: 'flex-end' }}>
                    <Pressable
                        onPress={() => setShowNewProject(true)}
                        style={({ pressed }) => ({
                            flexDirection: 'row',
                            alignItems: 'center',
                            paddingHorizontal: 12,
                            paddingVertical: 6,
                            borderRadius: 8,
                            backgroundColor: pressed
                                ? theme.colors.surfacePressed
                                : theme.colors.button.primary.background,
                        })}
                    >
                        <Ionicons name="add" size={18} color={theme.colors.button.primary.tint} />
                        <Text style={{
                            fontSize: 14,
                            color: theme.colors.button.primary.tint,
                            marginLeft: 4,
                            ...Typography.default('semiBold'),
                        }}>
                            {t('persona.newProject')}
                        </Text>
                    </Pressable>
                </View>
            </View>

            {loading && allPersonas.length === 0 ? (
                <View style={{ padding: 40, alignItems: 'center' }}>
                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                </View>
            ) : allPersonas.length === 0 ? (
                <View style={{
                    maxWidth: layout.maxWidth,
                    alignSelf: 'center',
                    width: '100%',
                    alignItems: 'center',
                    paddingVertical: 40,
                    paddingHorizontal: 20,
                }}>
                    <Ionicons name="sparkles-outline" size={48} color={theme.colors.textSecondary} />
                    <Text style={{
                        marginTop: 12,
                        fontSize: 16,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        ...Typography.default(),
                    }}>
                        {t('persona.emptyTitle')}
                    </Text>
                    <Text style={{
                        marginTop: 8,
                        fontSize: 14,
                        color: theme.colors.textSecondary,
                        textAlign: 'center',
                        lineHeight: 20,
                        ...Typography.default(),
                    }}>
                        {t('persona.emptyDescription')}
                    </Text>
                </View>
            ) : (
                <View style={{
                    maxWidth: layout.maxWidth,
                    alignSelf: 'center',
                    width: '100%',
                }}>
                    {projectGroups.map((group) => {
                        const collapsed = collapsedGroups.has(group.key);
                        return (
                            <View key={group.key || '__unlinked'}>
                                <ProjectSectionHeader
                                    label={group.label}
                                    collapsed={collapsed}
                                    onToggle={() => toggleGroup(group.key)}
                                    isUnlinked={!group.key}
                                    onDeleteProject={group.key ? () => handleDeleteProject(group) : undefined}
                                    deletingProject={group.key ? deletingProjectKeys.has(group.key) : false}
                                    onAddChat={group.key ? () => handleNewTaskWithVendor(group.key) : undefined}
                                />
                                {!collapsed && (
                                    <View style={{ paddingLeft: 16 }}>
                                        {renderPersonaSection(group.personas)}
                                    </View>
                                )}
                            </View>
                        );
                    })}
                </View>
            )}

            <NewProjectModal
                visible={showNewProject}
                machineId={machineId}
                homeDir={selectedMachine?.metadata?.homeDir}
                onClose={() => setShowNewProject(false)}
                onCreate={async (workdir) => {
                    // Close the project modal first, then show vendor selector
                    setShowNewProject(false);
                    handleNewTaskWithVendor(workdir);
                }}
            />

            <CreateWorkspaceModal
                visible={showCreateWorkspace}
                machineId={machineId}
                workdir={workspaceWorkdir}
                onClose={() => setShowCreateWorkspace(false)}
                onCreate={handleCreateWorkspace}
            />

            {avatarPickerPersona && (
                <AvatarPickerModal
                    visible={true}
                    currentAvatarId={avatarPickerPersona.avatarId}
                    onSelect={async (avatarId) => {
                        try {
                            await updatePersona({ id: avatarPickerPersona.id, avatarId });
                        } catch (err) {
                            console.error('Failed to update avatar:', err);
                        }
                    }}
                    onClose={() => setAvatarPickerPersona(null)}
                />
            )}

        </ItemList>
    );
});
