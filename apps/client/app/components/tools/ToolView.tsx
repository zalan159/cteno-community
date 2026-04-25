import * as React from 'react';
import { View, TouchableOpacity } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Ionicons, Octicons } from '@expo/vector-icons';
import { getToolViewComponent } from './views/_all';
import { Message, ToolCall } from '@/sync/typesMessage';
import { CodeView } from '../CodeView';
import { ToolSectionView } from './ToolSectionView';
import { ToolError } from './ToolError';
import { knownTools } from '@/components/tools/knownTools';
import { Metadata } from '@/sync/storageTypes';
import { useRouter } from 'expo-router';
import { PermissionFooter } from './PermissionFooter';
import { parseToolUseError } from '@/utils/toolErrorParser';
import { formatMCPTitle } from './views/MCPToolView';
import { t } from '@/text';
import { Text } from '@/components/StyledText';
import { sessionSendToBackground } from '@/sync/ops';
import { HostToolBadge } from './HostToolBadge';
import { getHostToolSubtitle, isHostOwnedTool } from './hostTool';
import { supportsHostBackgroundTransfer } from '@/utils/hostBackgroundTransfer';

interface ToolViewProps {
    metadata: Metadata | null;
    tool: ToolCall;
    messages?: Message[];
    onPress?: () => void;
    sessionId?: string;
    messageId?: string;
}

export const ToolView = React.memo<ToolViewProps>((props) => {
    const { tool, onPress, sessionId, messageId } = props;
    const router = useRouter();
    const { theme } = useUnistyles();
    const [sendingToBackground, setSendingToBackground] = React.useState(false);

    const canSendToBackground = !!sessionId && supportsHostBackgroundTransfer(props.metadata, tool);
    const isHostOwned = isHostOwnedTool(tool);

    const handleSendToBackground = React.useCallback(async () => {
        if (!sessionId || !tool.callId || sendingToBackground) return;
        setSendingToBackground(true);
        try {
            await sessionSendToBackground(sessionId, tool.callId);
        } catch (e) {
            console.warn('send-to-background failed:', e);
        }
        // Don't reset sendingToBackground - the tool state will change via reducer
    }, [sessionId, tool.callId, sendingToBackground]);

    // Create default onPress handler for navigation
    const handlePress = React.useCallback(() => {
        if (onPress) {
            onPress();
        } else if (sessionId && messageId) {
            router.push(`/session/${sessionId}/message/${messageId}`);
        }
    }, [onPress, sessionId, messageId, router]);

    // Enable pressable if either onPress is provided or we have navigation params
    const isPressable = !!(onPress || (sessionId && messageId));

    // Guard against undefined tool.name (can happen with incomplete ACP messages)
    const toolName = tool.name || 'unknown';

    let knownTool = knownTools[toolName as keyof typeof knownTools] as any;
    const tip = React.useMemo(() => {
        if (!knownTool || typeof knownTool.extractTip !== 'function') {
            return null;
        }
        return knownTool.extractTip({ tool, metadata: props.metadata });
    }, [
        knownTool,
        props.metadata?.flavor,
        props.metadata?.vendor,
        tool.callId,
        tool.input,
        tool.name,
        tool.state,
    ]);

    // Hidden tools (e.g. Claude CLI's ToolSearch) render nothing.
    if (knownTool?.hidden) {
        return null;
    }

    let description: string | null = null;
    let status: string | null = null;
    let minimal = false;
    let icon = <Ionicons name="construct-outline" size={18} color={theme.colors.textSecondary} />;
    let noStatus = false;
    let hideDefaultError = false;

    // For Gemini: unknown tools should be rendered as minimal (hidden)
    // This prevents showing raw INPUT/OUTPUT for internal Gemini tools
    // that we haven't explicitly added to knownTools
    const isGemini = props.metadata?.flavor === 'gemini';
    if (!knownTool && isGemini) {
        minimal = true;
    }

    // Extract status first to potentially use as title
    if (knownTool && typeof knownTool.extractStatus === 'function') {
        const state = knownTool.extractStatus({ tool, metadata: props.metadata });
        if (typeof state === 'string' && state) {
            status = state;
        }
    }

    // Handle optional title and function type
    let toolTitle = toolName;

    // Special handling for MCP tools
    if (toolName.startsWith('mcp__')) {
        toolTitle = formatMCPTitle(toolName);
        icon = <Ionicons name="extension-puzzle-outline" size={18} color={theme.colors.textSecondary} />;
        minimal = true;
    } else if (knownTool?.title) {
        if (typeof knownTool.title === 'function') {
            toolTitle = knownTool.title({ tool, metadata: props.metadata });
        } else {
            toolTitle = knownTool.title;
        }
    }

    if (knownTool && typeof knownTool.extractSubtitle === 'function') {
        const subtitle = knownTool.extractSubtitle({ tool, metadata: props.metadata });
        if (typeof subtitle === 'string' && subtitle) {
            description = subtitle;
        }
    }
    description = getHostToolSubtitle(tool, description);
    if (knownTool && knownTool.minimal !== undefined) {
        if (typeof knownTool.minimal === 'function') {
            minimal = knownTool.minimal({ tool, metadata: props.metadata, messages: props.messages });
        } else {
            minimal = knownTool.minimal;
        }
    }
    
    // Special handling for CodexBash to determine icon based on parsed_cmd
    if (toolName === 'CodexBash' && tool.input?.parsed_cmd && Array.isArray(tool.input.parsed_cmd) && tool.input.parsed_cmd.length > 0) {
        const parsedCmd = tool.input.parsed_cmd[0];
        if (parsedCmd.type === 'read') {
            icon = <Octicons name="eye" size={18} color={theme.colors.text} />;
        } else if (parsedCmd.type === 'write') {
            icon = <Octicons name="file-diff" size={18} color={theme.colors.text} />;
        } else {
            icon = <Octicons name="terminal" size={18} color={theme.colors.text} />;
        }
    } else if (knownTool && typeof knownTool.icon === 'function') {
        icon = knownTool.icon(18, theme.colors.text);
    }
    
    if (knownTool && typeof knownTool.noStatus === 'boolean') {
        noStatus = knownTool.noStatus;
    }
    if (knownTool && typeof knownTool.hideDefaultError === 'boolean') {
        hideDefaultError = knownTool.hideDefaultError;
    }

    let statusIcon = null;

    let isToolUseError = false;
    if (tool.state === 'error' && tool.result && parseToolUseError(tool.result).isToolUseError) {
        isToolUseError = true;
        console.log('isToolUseError', tool.result);
    }

    // Check permission status first for denied/canceled states
    if (tool.permission && (tool.permission.status === 'denied' || tool.permission.status === 'canceled')) {
        statusIcon = <Ionicons name="remove-circle-outline" size={20} color={theme.colors.textSecondary} />;
    } else if (isToolUseError) {
        statusIcon = <Ionicons name="remove-circle-outline" size={20} color={theme.colors.textSecondary} />;
        hideDefaultError = true;
        minimal = true;
    } else {
        switch (tool.state) {
            case 'running':
                break;
            case 'completed':
                // if (!noStatus) {
                //     statusIcon = <Ionicons name="checkmark-circle" size={20} color="#34C759" />;
                // }
                break;
            case 'error':
                statusIcon = <Ionicons name="alert-circle-outline" size={20} color={theme.colors.warning} />;
                break;
        }
    }

    return (
        <>
            <View style={styles.container}>
                {isPressable ? (
                    <TouchableOpacity style={styles.header} onPress={handlePress} activeOpacity={0.8}>
                        <View style={styles.headerLeft}>
                            <View style={styles.iconContainer}>
                                {icon}
                            </View>
                            <View style={styles.titleContainer}>
                                <View style={styles.titleRow}>
                                    <Text style={styles.toolName} numberOfLines={1}>
                                        {toolTitle}
                                        {status ? <Text style={styles.status}>{` ${status}`}</Text> : null}
                                    </Text>
                                    {isHostOwned ? <HostToolBadge /> : null}
                                </View>
                                {description && (
                                    <Text style={styles.toolDescription} numberOfLines={1}>
                                        {description}
                                    </Text>
                                )}
                            </View>
                            {canSendToBackground && (
                                <TouchableOpacity onPress={handleSendToBackground} disabled={sendingToBackground} activeOpacity={0.6} style={styles.bgIconBtn}>
                                    <Ionicons name="arrow-redo-outline" size={18} color={theme.colors.textSecondary} />
                                </TouchableOpacity>
                            )}
                            {statusIcon}
                        </View>
                    </TouchableOpacity>
                ) : (
                    <View style={styles.header}>
                        <View style={styles.headerLeft}>
                            <View style={styles.iconContainer}>
                                {icon}
                            </View>
                            <View style={styles.titleContainer}>
                                <View style={styles.titleRow}>
                                    <Text style={styles.toolName} numberOfLines={1}>
                                        {toolTitle}
                                        {status ? <Text style={styles.status}>{` ${status}`}</Text> : null}
                                    </Text>
                                    {isHostOwned ? <HostToolBadge /> : null}
                                </View>
                                {description && (
                                    <Text style={styles.toolDescription} numberOfLines={1}>
                                        {description}
                                    </Text>
                                )}
                            </View>
                            {canSendToBackground && (
                                <TouchableOpacity onPress={handleSendToBackground} disabled={sendingToBackground} activeOpacity={0.6} style={styles.bgIconBtn}>
                                    <Ionicons name="arrow-redo-outline" size={18} color={theme.colors.textSecondary} />
                                </TouchableOpacity>
                            )}
                            {statusIcon}
                        </View>
                    </View>
                )}

                {/* Content area - either custom children or tool-specific view */}
                {(() => {
                    // Check if minimal first - minimal tools don't show content
                    if (minimal) {
                        return null;
                    }

                    // Try to use a specific tool view component first
                    const SpecificToolView = getToolViewComponent(toolName);
                    if (SpecificToolView) {
                        return (
                            <View style={styles.content}>
                                <SpecificToolView tool={tool} metadata={props.metadata} messages={props.messages ?? []} sessionId={sessionId} />
                                {tool.state === 'error' && tool.result &&
                                    !(tool.permission && (tool.permission.status === 'denied' || tool.permission.status === 'canceled')) &&
                                    !hideDefaultError && (
                                        <ToolError message={String(tool.result)} />
                                    )}
                            </View>
                        );
                    }

                    // Show error state if present (but not for denied/canceled permissions and not when hideDefaultError is true)
                    if (tool.state === 'error' && tool.result &&
                        !(tool.permission && (tool.permission.status === 'denied' || tool.permission.status === 'canceled')) &&
                        !isToolUseError) {
                        return (
                            <View style={styles.content}>
                                <ToolError message={String(tool.result)} />
                            </View>
                        );
                    }

                    // Fall back to default view
                    return (
                        <View style={styles.content}>
                            {/* Default content when no custom view available */}
                            {tool.input && (
                                <ToolSectionView title={t('toolView.input')}>
                                    <CodeView code={JSON.stringify(tool.input, null, 2)} />
                                </ToolSectionView>
                            )}

                            {tool.state === 'completed' && tool.result && (
                                <ToolSectionView title={t('toolView.output')}>
                                    <CodeView
                                        code={typeof tool.result === 'string' ? tool.result : JSON.stringify(tool.result, null, 2)}
                                    />
                                </ToolSectionView>
                            )}
                        </View>
                    );
                })()}

                {/* Pending permissions render as the session input gate, not inside tool cards. */}
                {/* AskUserQuestion has its own Submit button UI - no permission footer needed */}
                {tool.permission && tool.permission.status !== 'pending' && sessionId && toolName !== 'AskUserQuestion' && (
                    <PermissionFooter permission={tool.permission} sessionId={sessionId} toolName={toolName} toolInput={tool.input} metadata={props.metadata} />
                )}

            </View>
            <Text style={[styles.cardTip, !tip && styles.cardTipEmpty]} numberOfLines={1}>
                {tip || ' '}
            </Text>
        </>
    );
});

const styles = StyleSheet.create((theme) => ({
    container: {
        backgroundColor: theme.colors.surfaceHigh,
        borderRadius: 8,
        marginVertical: 4,
        overflow: 'hidden'
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: 12,
        backgroundColor: theme.colors.surfaceHighest,
    },
    headerLeft: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        flex: 1,
    },
    iconContainer: {
        width: 24,
        height: 24,
        alignItems: 'center',
        justifyContent: 'center',
    },
    titleContainer: {
        flex: 1,
    },
    titleRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    cardTip: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        fontStyle: 'italic',
        lineHeight: 16,
        opacity: 0.7,
        marginTop: 4,
        marginBottom: 2,
        paddingHorizontal: 4,
    },
    cardTipEmpty: {
        opacity: 0,
    },
    bgIconBtn: {
        padding: 4,
        marginLeft: 4,
    },
    toolName: {
        flexShrink: 1,
        fontSize: 14,
        fontWeight: '500',
        color: theme.colors.text,
    },
    status: {
        fontWeight: '400',
        opacity: 0.3,
        fontSize: 15,
    },
    toolDescription: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        marginTop: 2,
    },
    content: {
        paddingHorizontal: 12,
        paddingTop: 8,
        overflow: 'visible'
    },
}));
