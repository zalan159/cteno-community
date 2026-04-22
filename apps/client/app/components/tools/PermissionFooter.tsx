import React, { useState } from 'react';
import { View, TouchableOpacity, ActivityIndicator, StyleSheet, Platform } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { sessionAllow, sessionDeny } from '@/sync/ops';
import { useUnistyles } from 'react-native-unistyles';
import { storage } from '@/sync/storage';
import { t } from '@/text';
import { Text } from '@/components/StyledText';

interface PermissionFooterProps {
    permission: {
        id: string;
        status: "pending" | "approved" | "denied" | "canceled";
        reason?: string;
        mode?: string;
        allowedTools?: string[];
        decision?: 'approved' | 'approved_for_session' | 'denied' | 'abort';
    };
    sessionId: string;
    toolName: string;
    toolInput?: any;
    metadata?: any;
}

interface GeminiPermissionOption {
    optionId: string;
    name?: string;
    kind?: string;
}

export const PermissionFooter: React.FC<PermissionFooterProps> = ({ permission, sessionId, toolName, toolInput, metadata }) => {
    const { theme } = useUnistyles();
    // loadingButton carries the key of the button currently spinning. For
    // the Codex / Claude branches it's always 'allow' / 'deny' / 'abort';
    // for the Gemini branch it's the vendor optionId (e.g. 'proceed_once').
    const [loadingButton, setLoadingButton] = useState<string | null>(null);
    const [loadingAllEdits, setLoadingAllEdits] = useState(false);
    const [loadingForSession, setLoadingForSession] = useState(false);

    // Check if this is a Codex session - check both metadata.flavor and tool name prefix
    const isCodex = metadata?.flavor === 'codex' || toolName.startsWith('Codex');

    // Gemini's ACP adapter stuffs the vendor-provided option list into
    // `tool_input._vendor_options` and sets `tool_input._vendor === 'gemini'`.
    // When present, render one button per option so the UI matches what
    // gemini-cli itself offers (proceed_once / proceed_always / cancel /
    // tool-specific additions). `handleGeminiOption` forwards the optionId
    // back verbatim via `sessionAllow/Deny`'s new `vendorOption` arg; the
    // Rust adapter uses it as `PermissionDecision::SelectedOption` to echo
    // to gemini unchanged.
    const geminiOptions: GeminiPermissionOption[] | null = (
        toolInput?._vendor === 'gemini' && Array.isArray(toolInput?._vendor_options)
            ? (toolInput._vendor_options as GeminiPermissionOption[]).filter(
                (o) => o && typeof o.optionId === 'string'
              )
            : null
    );
    const isGemini = !!(geminiOptions && geminiOptions.length > 0);

    const handleGeminiOption = async (opt: GeminiPermissionOption) => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingForSession) return;
        setLoadingButton(opt.optionId);
        try {
            const kind = opt.kind ?? '';
            const isReject = kind.startsWith('reject_') || opt.optionId === 'cancel';
            if (isReject) {
                // 'abort' so the turn stops cleanly; the optionId carries the
                // real gemini semantic so the adapter forwards `cancel` (or
                // whatever the server advertised) unchanged.
                await sessionDeny(sessionId, permission.id, undefined, undefined, 'abort', opt.optionId);
            } else {
                await sessionAllow(sessionId, permission.id, undefined, undefined, undefined, opt.optionId);
            }
        } catch (error) {
            console.error('Failed to respond to gemini permission option:', error);
        } finally {
            setLoadingButton(null);
        }
    };

    const handleApprove = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingAllEdits || loadingForSession) return;

        setLoadingButton('allow');
        try {
            await sessionAllow(sessionId, permission.id);
        } catch (error) {
            console.error('Failed to approve permission:', error);
        } finally {
            setLoadingButton(null);
        }
    };

    const handleApproveAllEdits = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingAllEdits || loadingForSession) return;

        setLoadingAllEdits(true);
        try {
            await sessionAllow(sessionId, permission.id, 'acceptEdits');
            // Update the session permission mode to 'acceptEdits' for future permissions
            storage.getState().updateSessionPermissionMode(sessionId, 'acceptEdits');
        } catch (error) {
            console.error('Failed to approve all edits:', error);
        } finally {
            setLoadingAllEdits(false);
        }
    };

    const handleApproveForSession = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingAllEdits || loadingForSession || !toolName) return;

        setLoadingForSession(true);
        try {
            // Special handling for Bash tool - include exact command
            let toolIdentifier = toolName;
            if (toolName === 'Bash' && toolInput?.command) {
                const command = toolInput.command;
                toolIdentifier = `Bash(${command})`;
            }
            
            await sessionAllow(sessionId, permission.id, undefined, [toolIdentifier], 'approved_for_session');
        } catch (error) {
            console.error('Failed to approve for session:', error);
        } finally {
            setLoadingForSession(false);
        }
    };

    const handleDeny = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingAllEdits || loadingForSession) return;

        setLoadingButton('deny');
        try {
            // Use 'abort' decision to stop the agent entirely, so the user can type feedback
            await sessionDeny(sessionId, permission.id, undefined, undefined, 'abort');
        } catch (error) {
            console.error('Failed to deny permission:', error);
        } finally {
            setLoadingButton(null);
        }
    };
    
    // Codex-specific handlers
    const handleCodexApprove = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingForSession) return;
        
        setLoadingButton('allow');
        try {
            await sessionAllow(sessionId, permission.id, undefined, undefined, 'approved');
        } catch (error) {
            console.error('Failed to approve permission:', error);
        } finally {
            setLoadingButton(null);
        }
    };
    
    const handleCodexApproveForSession = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingForSession) return;
        
        setLoadingForSession(true);
        try {
            await sessionAllow(sessionId, permission.id, undefined, undefined, 'approved_for_session');
        } catch (error) {
            console.error('Failed to approve for session:', error);
        } finally {
            setLoadingForSession(false);
        }
    };
    
    const handleCodexAbort = async () => {
        if (permission.status !== 'pending' || loadingButton !== null || loadingForSession) return;
        
        setLoadingButton('abort');
        try {
            await sessionDeny(sessionId, permission.id, undefined, undefined, 'abort');
        } catch (error) {
            console.error('Failed to abort permission:', error);
        } finally {
            setLoadingButton(null);
        }
    };

    const isApproved = permission.status === 'approved';
    const isDenied = permission.status === 'denied' || permission.status === 'canceled';
    const isPending = permission.status === 'pending';

    // Helper function to check if tool matches allowed pattern
    const isToolAllowed = (toolName: string, toolInput: any, allowedTools: string[] | undefined): boolean => {
        if (!allowedTools) return false;
        
        // Direct match for non-Bash tools
        if (allowedTools.includes(toolName)) return true;
        
        // For Bash, check exact command match
        if (toolName === 'Bash' && toolInput?.command) {
            const command = toolInput.command;
            return allowedTools.includes(`Bash(${command})`);
        }
        
        return false;
    };

    // Detect which button was used based on mode (for Claude) or decision (for Codex)
    const isApprovedViaAllow = isApproved && permission.mode !== 'acceptEdits' && !isToolAllowed(toolName, toolInput, permission.allowedTools);
    const isApprovedViaAllEdits = isApproved && permission.mode === 'acceptEdits';
    const isApprovedForSession = isApproved && isToolAllowed(toolName, toolInput, permission.allowedTools);
    
    // Codex-specific status detection with fallback
    const isCodexApproved = isCodex && isApproved && (permission.decision === 'approved' || !permission.decision);
    const isCodexApprovedForSession = isCodex && isApproved && permission.decision === 'approved_for_session';
    const isCodexAborted = isCodex && isDenied && permission.decision === 'abort';

    const styles = StyleSheet.create({
        container: {
            paddingHorizontal: 12,
            paddingVertical: 8,
            justifyContent: 'center',
        },
        buttonContainer: {
            flexDirection: 'column',
            gap: 4,
            alignItems: 'flex-start',
        },
        button: {
            paddingHorizontal: 12,
            paddingVertical: 8,
            borderRadius: 1,
            backgroundColor: 'transparent',
            alignItems: 'flex-start',
            justifyContent: 'center',
            minHeight: 32,
            borderLeftWidth: 3,
            borderLeftColor: 'transparent',
            alignSelf: 'stretch',
        },
        buttonAllow: {
            backgroundColor: 'transparent',
        },
        buttonDeny: {
            backgroundColor: 'transparent',
        },
        buttonAllowAll: {
            backgroundColor: 'transparent',
        },
        buttonSelected: {
            backgroundColor: 'transparent',
            borderLeftColor: theme.colors.text,
        },
        buttonInactive: {
            opacity: 0.3,
        },
        buttonContent: {
            flexDirection: 'row',
            alignItems: 'center',
            gap: 4,
            minHeight: 20,
        },
        icon: {
            marginRight: 2,
        },
        buttonText: {
            fontSize: 14,
            fontWeight: '400',
            color: theme.colors.textSecondary,
        },
        buttonTextAllow: {
            color: theme.colors.permissionButton.allow.background,
            fontWeight: '500',
        },
        buttonTextDeny: {
            color: theme.colors.permissionButton.deny.background,
            fontWeight: '500',
        },
        buttonTextAllowAll: {
            color: theme.colors.permissionButton.allowAll.background,
            fontWeight: '500',
        },
        buttonTextSelected: {
            color: theme.colors.text,
            fontWeight: '500',
        },
        buttonForSession: {
            backgroundColor: 'transparent',
        },
        buttonTextForSession: {
            color: theme.colors.permissionButton.allowAll.background,
            fontWeight: '500',
        },
        loadingIndicatorAllow: {
            color: theme.colors.permissionButton.allow.background,
        },
        loadingIndicatorDeny: {
            color: theme.colors.permissionButton.deny.background,
        },
        loadingIndicatorAllowAll: {
            color: theme.colors.permissionButton.allowAll.background,
        },
        loadingIndicatorForSession: {
            color: theme.colors.permissionButton.allowAll.background,
        },
        iconApproved: {
            color: theme.colors.permissionButton.allow.background,
        },
        iconDenied: {
            color: theme.colors.permissionButton.deny.background,
        },
    });

    // Render Gemini buttons if the adapter surfaced a vendor option list.
    // Button styling:
    //   - allow_once / allow_always kinds → allow color
    //   - reject_once / reject_always kinds (or optionId === 'cancel') → deny color
    //   - anything else → neutral (shouldn't happen with current gemini but
    //     we don't want to drop options just because a newer CLI adds a kind)
    if (isGemini) {
        return (
            <View style={styles.container}>
                <View style={styles.buttonContainer}>
                    {geminiOptions!.map((opt) => {
                        const kind = opt.kind ?? '';
                        const isAllow = kind.startsWith('allow_');
                        const isReject = kind.startsWith('reject_') || opt.optionId === 'cancel';
                        const isSelected =
                            isPending === false &&
                            permission.status === 'approved' &&
                            !isReject;
                        const label = opt.name || opt.optionId;
                        const loadingColor = isReject
                            ? styles.loadingIndicatorDeny.color
                            : styles.loadingIndicatorAllow.color;
                        const textStyle = [
                            styles.buttonText,
                            isPending && isReject && styles.buttonTextDeny,
                            isPending && isAllow && !isReject && styles.buttonTextAllow,
                            isSelected && styles.buttonTextSelected,
                        ];
                        const buttonStyle = [
                            styles.button,
                            isPending && isReject && styles.buttonDeny,
                            isPending && !isReject && styles.buttonAllow,
                            isSelected && styles.buttonSelected,
                            (!isPending && !isSelected) && styles.buttonInactive,
                        ];
                        return (
                            <TouchableOpacity
                                key={opt.optionId}
                                style={buttonStyle}
                                onPress={() => handleGeminiOption(opt)}
                                disabled={!isPending || loadingButton !== null}
                                activeOpacity={isPending ? 0.7 : 1}
                            >
                                {loadingButton === opt.optionId && isPending ? (
                                    <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                        <ActivityIndicator size={Platform.OS === 'ios' ? 'small' : 14 as any} color={loadingColor} />
                                    </View>
                                ) : (
                                    <View style={styles.buttonContent}>
                                        <Text style={textStyle} numberOfLines={1} ellipsizeMode="tail">
                                            {label}
                                        </Text>
                                    </View>
                                )}
                            </TouchableOpacity>
                        );
                    })}
                </View>
            </View>
        );
    }

    // Render Codex buttons if this is a Codex session
    if (isCodex) {
        return (
            <View style={styles.container}>
                <View style={styles.buttonContainer}>
                    {/* Codex: Yes button */}
                    <TouchableOpacity
                        style={[
                            styles.button,
                            isPending && styles.buttonAllow,
                            isCodexApproved && styles.buttonSelected,
                            (isCodexAborted || isCodexApprovedForSession) && styles.buttonInactive
                        ]}
                        onPress={handleCodexApprove}
                        disabled={!isPending || loadingButton !== null || loadingForSession}
                        activeOpacity={isPending ? 0.7 : 1}
                    >
                        {loadingButton === 'allow' && isPending ? (
                            <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorAllow.color} />
                            </View>
                        ) : (
                            <View style={styles.buttonContent}>
                                <Text style={[
                                    styles.buttonText,
                                    isPending && styles.buttonTextAllow,
                                    isCodexApproved && styles.buttonTextSelected
                                ]} numberOfLines={1} ellipsizeMode="tail">
                                    {t('common.yes')}
                                </Text>
                            </View>
                        )}
                    </TouchableOpacity>

                    {/* Codex: Yes, and don't ask for a session button */}
                    <TouchableOpacity
                        style={[
                            styles.button,
                            isPending && styles.buttonForSession,
                            isCodexApprovedForSession && styles.buttonSelected,
                            (isCodexAborted || isCodexApproved) && styles.buttonInactive
                        ]}
                        onPress={handleCodexApproveForSession}
                        disabled={!isPending || loadingButton !== null || loadingForSession}
                        activeOpacity={isPending ? 0.7 : 1}
                    >
                        {loadingForSession && isPending ? (
                            <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorForSession.color} />
                            </View>
                        ) : (
                            <View style={styles.buttonContent}>
                                <Text style={[
                                    styles.buttonText,
                                    isPending && styles.buttonTextForSession,
                                    isCodexApprovedForSession && styles.buttonTextSelected
                                ]} numberOfLines={1} ellipsizeMode="tail">
                                    {t('codex.permissions.yesForSession')}
                                </Text>
                            </View>
                        )}
                    </TouchableOpacity>

                    {/* Codex: Stop, and explain what to do button */}
                    <TouchableOpacity
                        style={[
                            styles.button,
                            isPending && styles.buttonDeny,
                            isCodexAborted && styles.buttonSelected,
                            (isCodexApproved || isCodexApprovedForSession) && styles.buttonInactive
                        ]}
                        onPress={handleCodexAbort}
                        disabled={!isPending || loadingButton !== null || loadingForSession}
                        activeOpacity={isPending ? 0.7 : 1}
                    >
                        {loadingButton === 'abort' && isPending ? (
                            <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorDeny.color} />
                            </View>
                        ) : (
                            <View style={styles.buttonContent}>
                                <Text style={[
                                    styles.buttonText,
                                    isPending && styles.buttonTextDeny,
                                    isCodexAborted && styles.buttonTextSelected
                                ]} numberOfLines={1} ellipsizeMode="tail">
                                    {t('codex.permissions.stopAndExplain')}
                                </Text>
                            </View>
                        )}
                    </TouchableOpacity>
                </View>
            </View>
        );
    }

    // Render Claude buttons (existing behavior)
    return (
        <View style={styles.container}>
            <View style={styles.buttonContainer}>
                <TouchableOpacity
                    style={[
                        styles.button,
                        isPending && styles.buttonAllow,
                        isApprovedViaAllow && styles.buttonSelected,
                        (isDenied || isApprovedViaAllEdits || isApprovedForSession) && styles.buttonInactive
                    ]}
                    onPress={handleApprove}
                    disabled={!isPending || loadingButton !== null || loadingAllEdits || loadingForSession}
                    activeOpacity={isPending ? 0.7 : 1}
                >
                    {loadingButton === 'allow' && isPending ? (
                        <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                            <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorAllow.color} />
                        </View>
                    ) : (
                        <View style={styles.buttonContent}>
                            <Text style={[
                                styles.buttonText,
                                isPending && styles.buttonTextAllow,
                                isApprovedViaAllow && styles.buttonTextSelected
                            ]} numberOfLines={1} ellipsizeMode="tail">
                                {t('common.yes')}
                            </Text>
                        </View>
                    )}
                </TouchableOpacity>

                {/* Allow All Edits button - only show for Edit and MultiEdit tools */}
                {(toolName === 'Edit' || toolName === 'MultiEdit' || toolName === 'Write' || toolName === 'NotebookEdit' || toolName === 'exit_plan_mode' || toolName === 'ExitPlanMode') && (
                    <TouchableOpacity
                        style={[
                            styles.button,
                            isPending && styles.buttonAllowAll,
                            isApprovedViaAllEdits && styles.buttonSelected,
                            (isDenied || isApprovedViaAllow || isApprovedForSession) && styles.buttonInactive
                        ]}
                        onPress={handleApproveAllEdits}
                        disabled={!isPending || loadingButton !== null || loadingAllEdits || loadingForSession}
                        activeOpacity={isPending ? 0.7 : 1}
                    >
                        {loadingAllEdits && isPending ? (
                            <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorAllowAll.color} />
                            </View>
                        ) : (
                            <View style={styles.buttonContent}>
                                <Text style={[
                                    styles.buttonText,
                                    isPending && styles.buttonTextAllowAll,
                                    isApprovedViaAllEdits && styles.buttonTextSelected
                                ]} numberOfLines={1} ellipsizeMode="tail">
                                    {t('claude.permissions.yesAllowAllEdits')}
                                </Text>
                            </View>
                        )}
                    </TouchableOpacity>
                )}

                {/* Allow for session button - only show for non-edit, non-exit-plan tools */}
                {toolName && toolName !== 'Edit' && toolName !== 'MultiEdit' && toolName !== 'Write' && toolName !== 'NotebookEdit' && toolName !== 'exit_plan_mode' && toolName !== 'ExitPlanMode' && (
                    <TouchableOpacity
                        style={[
                            styles.button,
                            isPending && styles.buttonForSession,
                            isApprovedForSession && styles.buttonSelected,
                            (isDenied || isApprovedViaAllow || isApprovedViaAllEdits) && styles.buttonInactive
                        ]}
                        onPress={handleApproveForSession}
                        disabled={!isPending || loadingButton !== null || loadingAllEdits || loadingForSession}
                        activeOpacity={isPending ? 0.7 : 1}
                    >
                        {loadingForSession && isPending ? (
                            <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                                <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorForSession.color} />
                            </View>
                        ) : (
                            <View style={styles.buttonContent}>
                                <Text style={[
                                    styles.buttonText,
                                    isPending && styles.buttonTextForSession,
                                    isApprovedForSession && styles.buttonTextSelected
                                ]} numberOfLines={1} ellipsizeMode="tail">
                                    {t('claude.permissions.yesForTool')}
                                </Text>
                            </View>
                        )}
                    </TouchableOpacity>
                )}

                <TouchableOpacity
                    style={[
                        styles.button,
                        isPending && styles.buttonDeny,
                        isDenied && styles.buttonSelected,
                        (isApproved) && styles.buttonInactive
                    ]}
                    onPress={handleDeny}
                    disabled={!isPending || loadingButton !== null || loadingAllEdits || loadingForSession}
                    activeOpacity={isPending ? 0.7 : 1}
                >
                    {loadingButton === 'deny' && isPending ? (
                        <View style={[styles.buttonContent, { width: 40, height: 20, justifyContent: 'center' }]}>
                            <ActivityIndicator size={Platform.OS === 'ios' ? "small" : 14 as any} color={styles.loadingIndicatorDeny.color} />
                        </View>
                    ) : (
                        <View style={styles.buttonContent}>
                            <Text style={[
                                styles.buttonText,
                                isPending && styles.buttonTextDeny,
                                isDenied && styles.buttonTextSelected
                            ]} numberOfLines={1} ellipsizeMode="tail">
                                {t('claude.permissions.noTellClaude')}
                            </Text>
                        </View>
                    )}
                </TouchableOpacity>
            </View>
        </View>
    );
};
