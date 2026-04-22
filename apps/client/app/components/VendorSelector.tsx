import * as Clipboard from 'expo-clipboard';
import { useRouter } from 'expo-router';
import React, { useEffect, useState } from 'react';
import { View, Pressable, Platform, Image as RNImage } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { Modal } from '@/modal';
import {
    listAvailableVendors,
    normalizeVendorList,
    probeVendorConnection,
    ResolvedVendorMeta,
    VendorMeta,
    VendorName,
} from '@/sync/ops';
import { openExternalUrl } from '@/utils/openExternalUrl';
import { getVendorIconSource } from '@/utils/vendorIcons';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        backgroundColor: theme.colors.surface,
        borderRadius: Platform.select({ default: 12, android: 16 }),
        marginBottom: 12,
        overflow: 'hidden',
    },
    title: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        marginBottom: 8,
        marginLeft: 16,
        marginTop: 12,
        ...Typography.default('semiBold'),
    },
    optionContainer: {
        flexDirection: 'row',
        alignItems: 'flex-start',
        paddingHorizontal: 16,
        paddingVertical: 12,
        minHeight: 64,
    },
    optionPressed: {
        backgroundColor: theme.colors.surfacePressed,
    },
    optionDisabled: {
        opacity: 0.45,
    },
    radioButton: {
        width: 20,
        height: 20,
        borderRadius: 10,
        borderWidth: 2,
        alignItems: 'center',
        justifyContent: 'center',
        marginRight: 12,
    },
    radioButtonActive: {
        borderColor: theme.colors.radio.active,
    },
    radioButtonInactive: {
        borderColor: theme.colors.radio.inactive,
    },
    radioButtonDot: {
        width: 8,
        height: 8,
        borderRadius: 4,
        backgroundColor: theme.colors.radio.dot,
    },
    optionBody: {
        flex: 1,
    },
    optionHeader: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
    },
    optionIdentity: {
        flexDirection: 'row',
        alignItems: 'center',
        flex: 1,
        minWidth: 0,
    },
    optionIcon: {
        width: 20,
        height: 20,
        marginRight: 8,
        marginTop: 1,
    },
    optionLabel: {
        fontSize: 16,
        color: theme.colors.text,
        ...Typography.default('regular'),
    },
    optionLabelLocked: {
        color: theme.colors.textSecondary,
    },
    statusChip: {
        borderRadius: 999,
        paddingHorizontal: 10,
        paddingVertical: 4,
        marginLeft: 8,
    },
    statusChipReady: {
        backgroundColor: 'rgba(52, 199, 89, 0.14)',
    },
    statusChipWarning: {
        backgroundColor: 'rgba(255, 59, 48, 0.10)',
    },
    statusChipMuted: {
        backgroundColor: theme.colors.surfaceHigh,
    },
    statusChipText: {
        fontSize: 12,
        ...Typography.default('semiBold'),
    },
    statusChipTextReady: {
        color: theme.colors.success,
    },
    statusChipTextWarning: {
        color: theme.colors.warningCritical,
    },
    statusChipTextMuted: {
        color: theme.colors.textSecondary,
    },
    statusDetail: {
        marginTop: 6,
        fontSize: 13,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
    actionRow: {
        flexDirection: 'row',
        flexWrap: 'wrap',
        gap: 8,
        paddingHorizontal: 48,
        paddingBottom: 12,
    },
    actionButton: {
        borderRadius: 999,
        paddingHorizontal: 12,
        paddingVertical: 8,
        borderWidth: 1,
    },
    actionButtonPrimary: {
        backgroundColor: theme.colors.text,
        borderColor: theme.colors.text,
    },
    actionButtonSecondary: {
        backgroundColor: theme.colors.surface,
        borderColor: theme.colors.divider,
    },
    actionButtonPressed: {
        opacity: 0.82,
    },
    actionButtonText: {
        fontSize: 13,
        ...Typography.default('semiBold'),
    },
    actionButtonTextPrimary: {
        color: theme.colors.surface,
    },
    actionButtonTextSecondary: {
        color: theme.colors.text,
    },
    divider: {
        height: Platform.select({ ios: 0.33, default: 0.5 }),
        backgroundColor: theme.colors.divider,
        marginLeft: 48,
    },
    emptyLabel: {
        paddingHorizontal: 16,
        paddingVertical: 14,
        color: theme.colors.textSecondary,
        fontSize: 14,
        ...Typography.default(),
    },
    workspaceRow: {
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: 16,
        paddingVertical: 14,
        minHeight: 56,
    },
    workspaceRowPressed: {
        backgroundColor: theme.colors.surfacePressed,
    },
    workspaceIconWrap: {
        width: 32,
        height: 32,
        borderRadius: 16,
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: theme.colors.surfaceHigh,
        marginRight: 12,
    },
    workspaceBody: {
        flex: 1,
    },
    workspaceLabel: {
        fontSize: 16,
        color: theme.colors.text,
        ...Typography.default('semiBold'),
    },
    workspaceDetail: {
        marginTop: 2,
        fontSize: 13,
        color: theme.colors.textSecondary,
        ...Typography.default(),
    },
}));

export interface VendorSelectorProps {
    /** The currently-selected vendor. */
    value: VendorName | null;
    /** Invoked when the user taps an available vendor row. */
    onChange: (vendor: VendorName) => void;
    /** Optional label shown above the vendor list. */
    title?: string;
    /**
     * Optional machineId to resolve the vendor list against. Falls back to
     * the mock vendor list when omitted or when the backend RPC is not yet
     * available.
     */
    machineId?: string | null;
    /** Optional pre-fetched vendors list (legacy or normalized shape). */
    vendors?: VendorMeta[];
    /**
     * If provided, render an extra "新建工作间" row at the bottom of the
     * selector. Invoked when the user taps that row.
     */
    onCreateWorkspace?: () => void;
}

const VENDOR_LABEL: Record<VendorName, string> = {
    cteno: 'Cteno',
    claude: 'Claude',
    codex: 'Codex',
    gemini: 'Gemini',
};

const VENDOR_INSTALL_COMMAND: Partial<Record<VendorName, string>> = {
    cteno: 'npm install -g cteno@latest',
    claude: 'npm install -g @anthropic-ai/claude-code',
    codex: 'npm install -g @openai/codex',
};

const VENDOR_SETUP_URL: Partial<Record<VendorName, string>> = {
    claude: 'https://docs.anthropic.com/claude-code',
    codex: 'https://github.com/openai/codex',
};

type VendorTone = 'ready' | 'warning' | 'muted';
type VendorActionId = 'install' | 'login' | 'retry-probe';

interface VendorActionDescriptor {
    id: VendorActionId;
    label: string;
    variant: 'primary' | 'secondary';
}

interface VendorPresentation {
    badge: string;
    detail: string;
    tone: VendorTone;
    canSelect: boolean;
    actions: VendorActionDescriptor[];
}

function getBaseVendorPresentation(vendor: ResolvedVendorMeta): VendorPresentation {
    if (!vendor.installed) {
        return {
            badge: 'Not installed',
            detail: 'Install the CLI on this machine to enable this vendor.',
            tone: 'muted',
            canSelect: false,
            actions: [
                {
                    id: 'install',
                    label: VENDOR_INSTALL_COMMAND[vendor.name]
                        ? 'Copy install command'
                        : 'View install steps',
                    variant: 'primary',
                },
            ],
        };
    }

    switch (vendor.status.authState) {
        case 'loggedIn':
        case 'notRequired':
            return {
                badge: 'Ready',
                detail: 'Installed and ready on this machine.',
                tone: 'ready',
                canSelect: true,
                actions: [],
            };
        case 'loggedOut':
            if (vendor.name === 'cteno') {
                return {
                    badge: 'Not logged in',
                    detail: 'Local community mode still works. Sign in to unlock cloud-backed features.',
                    tone: 'warning',
                    canSelect: true,
                    actions: [{ id: 'login', label: 'Sign in', variant: 'primary' }],
                };
            }
            return {
                badge: 'Login required',
                detail: 'Finish vendor login before starting sessions with this CLI.',
                tone: 'warning',
                canSelect: false,
                actions: [{ id: 'login', label: 'View login steps', variant: 'primary' }],
            };
        default:
            return {
                badge: 'Installed',
                detail: 'CLI detected. Login status is not reported yet.',
                tone: 'muted',
                canSelect: true,
                actions: [],
            };
    }
}

function getVendorPresentation(vendor: ResolvedVendorMeta): VendorPresentation {
    const base = getBaseVendorPresentation(vendor);

    // Only let connection state override the presentation once the vendor is
    // actually runnable (installed + has a login story). Not-installed /
    // login-required rows keep their existing copy.
    if (!vendor.installed) return base;

    const connection = vendor.status.connection;
    switch (connection.state) {
        case 'connected': {
            // Preserve the tone/copy inherited from authState. For the Ready
            // path we append the latency; other paths (e.g. loggedOut + cteno)
            // stay untouched.
            if (base.tone === 'ready' && typeof connection.latencyMs === 'number') {
                return {
                    ...base,
                    detail: `${base.detail} · ${connection.latencyMs}ms`,
                };
            }
            return base;
        }
        case 'probing':
            return {
                badge: 'Connecting…',
                detail: 'Warming up the agent runtime. This usually takes a moment.',
                tone: 'muted',
                // Don't block UX — spawn will hold until the probe completes.
                canSelect: true,
                actions: [],
            };
        case 'failed':
            return {
                badge: 'Connection failed',
                detail: connection.reason ?? 'Connection check failed. Retry to recheck.',
                tone: 'warning',
                canSelect: false,
                actions: [{ id: 'retry-probe', label: 'Retry', variant: 'primary' }],
            };
        case 'unknown':
        default:
            return base;
    }
}

export function VendorSelector({
    value,
    onChange,
    title = 'Agent backend',
    machineId,
    vendors: initialVendors,
    onCreateWorkspace,
}: VendorSelectorProps) {
    const styles = stylesheet;
    const { theme } = useUnistyles();
    const router = useRouter();
    const [vendors, setVendors] = useState<ResolvedVendorMeta[]>(
        initialVendors ? normalizeVendorList(initialVendors) : []
    );
    const [loading, setLoading] = useState(!initialVendors);

    const copyCommand = async (command: string) => {
        try {
            await Clipboard.setStringAsync(command);
            Modal.alert('Copied', command);
        } catch (error) {
            Modal.alert('Error', 'Failed to copy the command.');
        }
    };

    const openSetupGuide = async (vendor: VendorName, titleText: string, fallbackText: string) => {
        const url = VENDOR_SETUP_URL[vendor];
        if (url) {
            try {
                await openExternalUrl(url);
                return;
            } catch (error) {
                console.warn('[VendorSelector] failed to open setup guide:', error);
            }
        }
        Modal.alert(titleText, fallbackText);
    };

    const handleVendorAction = async (vendor: ResolvedVendorMeta, actionId: VendorActionId) => {
        if (actionId === 'retry-probe') {
            try {
                const next = await probeVendorConnection(machineId ?? null, vendor.name);
                setVendors((list) =>
                    list.map((v) =>
                        v.name === vendor.name
                            ? {
                                  ...v,
                                  status: {
                                      ...v.status,
                                      connection: next,
                                  },
                              }
                            : v
                    )
                );
            } catch (error) {
                Modal.alert('Probe failed', String(error));
            }
            return;
        }

        if (actionId === 'install') {
            const installCommand = VENDOR_INSTALL_COMMAND[vendor.name];
            if (installCommand) {
                await copyCommand(installCommand);
                return;
            }
            await openSetupGuide(
                vendor.name,
                `${VENDOR_LABEL[vendor.name]} setup`,
                `Install the ${VENDOR_LABEL[vendor.name]} CLI and ensure \`${vendor.name}\` is available on PATH.`
            );
            return;
        }

        if (vendor.name === 'cteno') {
            router.push('/settings/account');
            return;
        }

        await openSetupGuide(
            vendor.name,
            `${VENDOR_LABEL[vendor.name]} login`,
            `Finish login in the ${VENDOR_LABEL[vendor.name]} CLI, then reopen this selector.`
        );
    };

    useEffect(() => {
        if (initialVendors) {
            setVendors(normalizeVendorList(initialVendors));
            setLoading(false);
            return;
        }
        let cancelled = false;
        const fetchVendors = () =>
            listAvailableVendors(machineId ?? null)
                .then((list) => {
                    if (!cancelled) setVendors(list);
                })
                .catch((error) => {
                    console.warn('[VendorSelector] refresh failed:', error);
                });
        // Initial fetch — surfaces whatever state the daemon has cached so
        // far (including connection.state='probing' while the preheat is
        // still running).
        fetchVendors().finally(() => {
            if (!cancelled) setLoading(false);
        });
        // Second fetch: boot-time preheat (Phase-1 backend) typically settles
        // within ~1s. Re-read after 1.2s so we pick up connected/failed
        // without waiting for a user action.
        const timer = setTimeout(() => {
            if (!cancelled) void fetchVendors();
        }, 1200);
        return () => {
            cancelled = true;
            clearTimeout(timer);
        };
    }, [machineId, initialVendors]);

    return (
        <View style={styles.container}>
            <Text style={styles.title}>{title}</Text>
            {loading && (
                <Text style={styles.emptyLabel}>Detecting installed CLIs…</Text>
            )}
            {!loading && vendors.length === 0 && (
                <Text style={styles.emptyLabel}>
                    No executor vendors reported by this machine.
                </Text>
            )}
            {vendors.map((vendor, idx) => {
                const isActive = value === vendor.name;
                const presentation = getVendorPresentation(vendor);
                const iconSource = getVendorIconSource(vendor.name);
                return (
                    <React.Fragment key={vendor.name}>
                        <View>
                            <Pressable
                                onPress={() => {
                                    if (presentation.canSelect) onChange(vendor.name);
                                }}
                                disabled={!presentation.canSelect}
                                style={({ pressed }) => [
                                    styles.optionContainer,
                                    pressed && presentation.canSelect && styles.optionPressed,
                                    !presentation.canSelect && styles.optionDisabled,
                                ]}
                            >
                                <View
                                    style={[
                                        styles.radioButton,
                                        isActive
                                            ? styles.radioButtonActive
                                            : styles.radioButtonInactive,
                                    ]}
                                >
                                    {isActive && <View style={styles.radioButtonDot} />}
                                </View>
                                <View style={styles.optionBody}>
                                    <View style={styles.optionHeader}>
                                        <View style={styles.optionIdentity}>
                                            <RNImage
                                                source={iconSource}
                                                style={styles.optionIcon}
                                                resizeMode="contain"
                                            />
                                            <Text
                                                style={[
                                                    styles.optionLabel,
                                                    !presentation.canSelect && styles.optionLabelLocked,
                                                ]}
                                            >
                                                {VENDOR_LABEL[vendor.name]}
                                            </Text>
                                        </View>
                                        <View
                                            style={[
                                                styles.statusChip,
                                                presentation.tone === 'ready'
                                                    ? styles.statusChipReady
                                                    : presentation.tone === 'warning'
                                                        ? styles.statusChipWarning
                                                        : styles.statusChipMuted,
                                            ]}
                                        >
                                            <Text
                                                style={[
                                                    styles.statusChipText,
                                                    presentation.tone === 'ready'
                                                        ? styles.statusChipTextReady
                                                        : presentation.tone === 'warning'
                                                            ? styles.statusChipTextWarning
                                                            : styles.statusChipTextMuted,
                                                ]}
                                            >
                                                {presentation.badge}
                                            </Text>
                                        </View>
                                    </View>
                                    <Text style={styles.statusDetail}>{presentation.detail}</Text>
                                </View>
                            </Pressable>
                            {presentation.actions.length > 0 && (
                                <View style={styles.actionRow}>
                                    {presentation.actions.map((action) => (
                                        <Pressable
                                            key={`${vendor.name}-${action.id}`}
                                            onPress={() => {
                                                void handleVendorAction(vendor, action.id);
                                            }}
                                            style={({ pressed }) => [
                                                styles.actionButton,
                                                action.variant === 'primary'
                                                    ? styles.actionButtonPrimary
                                                    : styles.actionButtonSecondary,
                                                pressed && styles.actionButtonPressed,
                                            ]}
                                        >
                                            <Text
                                                style={[
                                                    styles.actionButtonText,
                                                    action.variant === 'primary'
                                                        ? styles.actionButtonTextPrimary
                                                        : styles.actionButtonTextSecondary,
                                                ]}
                                            >
                                                {action.label}
                                            </Text>
                                        </Pressable>
                                    ))}
                                </View>
                            )}
                        </View>
                        {idx < vendors.length - 1 && <View style={styles.divider} />}
                    </React.Fragment>
                );
            })}
            {onCreateWorkspace && (
                <>
                    {vendors.length > 0 && <View style={styles.divider} />}
                    <Pressable
                        onPress={onCreateWorkspace}
                        style={({ pressed }) => [
                            styles.workspaceRow,
                            pressed && styles.workspaceRowPressed,
                        ]}
                    >
                        <View style={styles.workspaceIconWrap}>
                            <Ionicons name="people-outline" size={18} color={theme.colors.text} />
                        </View>
                        <View style={styles.workspaceBody}>
                            <Text style={styles.workspaceLabel}>新建工作间</Text>
                            <Text style={styles.workspaceDetail}>多角色协作模板，按流程拆分任务与产物。</Text>
                        </View>
                        <Ionicons name="chevron-forward" size={18} color={theme.colors.textSecondary} />
                    </Pressable>
                </>
            )}
        </View>
    );
}
