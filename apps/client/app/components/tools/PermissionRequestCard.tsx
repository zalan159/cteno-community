import { Ionicons, Octicons } from '@expo/vector-icons';
import * as React from 'react';
import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import type { Metadata } from '@/sync/storageTypes';
import type { ToolCall } from '@/sync/typesMessage';
import { stringifyToolCommand } from '@/utils/toolCommand';
import { resolvePath } from '@/utils/pathUtils';
import { knownTools } from './knownTools';
import { formatMCPTitle } from './views/MCPToolView';
import { PermissionFooter } from './PermissionFooter';

type PendingPermission = {
    id: string;
    tool: string;
    arguments: any;
    createdAt?: number | null;
};

type PermissionRequestCardProps = {
    sessionId: string;
    pendingPermission: PendingPermission;
    metadata?: Metadata | null;
    variant?: 'embedded' | 'card';
};

type Detail = {
    label: string;
    value: string;
    monospace?: boolean;
};

function compact(value: unknown, max = 180): string | null {
    if (value === null || value === undefined) return null;
    let text: string;
    if (typeof value === 'string') {
        text = value.trim();
    } else if (typeof value === 'number' || typeof value === 'boolean') {
        text = String(value);
    } else {
        try {
            text = JSON.stringify(value);
        } catch {
            return null;
        }
    }
    if (!text) return null;
    return text.length > max ? `${text.slice(0, max - 1)}...` : text;
}

function firstString(input: any, keys: string[]): string | null {
    for (const key of keys) {
        const value = compact(input?.[key]);
        if (value) return value;
    }
    return null;
}

function firstPath(input: any, metadata: Metadata | null | undefined): string | null {
    const value =
        firstString(input, ['file_path', 'target_file', 'path', 'notebook_path']) ||
        compact(input?.locations?.[0]?.path) ||
        compact(input?.items?.[0]?.path);
    return value ? resolvePath(value, metadata ?? null) : null;
}

function commandFromInput(input: any): string | null {
    return (
        stringifyToolCommand(input?.command) ||
        compact(input?.cmd) ||
        compact(input?.parsed_cmd?.[0]?.cmd) ||
        compact(input?.parsed_cmd?.[0]?.command)
    );
}

function fallbackInputSummary(input: any): string | null {
    if (!input || typeof input !== 'object') return compact(input);
    const hiddenKeys = new Set(['_vendor', '_vendor_options']);
    const entries = Object.entries(input)
        .filter(([key, value]) => !hiddenKeys.has(key) && value !== undefined && value !== null)
        .slice(0, 3)
        .map(([key, value]) => {
            const text = compact(value, 80);
            return text ? `${key}: ${text}` : null;
        })
        .filter((value): value is string => !!value);
    return entries.length > 0 ? entries.join(' · ') : null;
}

function toolDisplay(toolName: string, input: any, metadata?: Metadata | null): { title: string; subtitle: string | null } {
    const tool: ToolCall = {
        name: toolName,
        state: 'running',
        input,
        createdAt: Date.now(),
        startedAt: null,
        completedAt: null,
        description: null,
    };

    if (toolName.startsWith('mcp__')) {
        return { title: formatMCPTitle(toolName), subtitle: toolName };
    }

    const knownTool = knownTools[toolName as keyof typeof knownTools] as any;
    if (!knownTool) {
        return { title: toolName, subtitle: null };
    }

    const title = typeof knownTool.title === 'function'
        ? knownTool.title({ tool, metadata: metadata ?? null })
        : knownTool.title || toolName;
    const subtitle = typeof knownTool.extractSubtitle === 'function'
        ? knownTool.extractSubtitle({ tool, metadata: metadata ?? null })
        : null;

    return {
        title: typeof title === 'string' && title ? title : toolName,
        subtitle: typeof subtitle === 'string' && subtitle ? subtitle : null,
    };
}

function buildDetails(toolName: string, input: any, metadata?: Metadata | null): Detail[] {
    const details: Detail[] = [];
    const command = commandFromInput(input);
    const path = firstPath(input, metadata);
    const cwd = firstString(input, ['cwd', 'working_dir']);
    const url = firstString(input, ['url', 'uri']);
    const query = firstString(input, ['query', 'pattern', 'regex']);
    const description = firstString(input, ['description', 'prompt', 'task']);

    if (command) details.push({ label: 'Command', value: command, monospace: true });
    if (path && path !== command) details.push({ label: 'Path', value: path, monospace: true });
    if (url) details.push({ label: 'URL', value: url, monospace: true });
    if (query) details.push({ label: 'Query', value: query, monospace: true });
    if (cwd) details.push({ label: 'CWD', value: cwd, monospace: true });
    if (Array.isArray(input?.edits)) details.push({ label: 'Edits', value: String(input.edits.length) });
    if (typeof input?.content === 'string') details.push({ label: 'Content', value: `${input.content.length} chars` });
    if (description) details.push({ label: 'Details', value: description });

    if (details.length === 0) {
        const summary = fallbackInputSummary(input);
        if (summary) details.push({ label: toolName, value: summary, monospace: true });
    }

    return details.slice(0, 5);
}

export function PermissionRequestCard({
    sessionId,
    pendingPermission,
    metadata,
    variant = 'embedded',
}: PermissionRequestCardProps) {
    const { theme } = useUnistyles();
    const input = pendingPermission.arguments;
    const display = toolDisplay(pendingPermission.tool, input, metadata);
    const details = buildDetails(pendingPermission.tool, input, metadata);
    const subtitle = display.subtitle
        ? `${pendingPermission.tool} · ${display.subtitle}`
        : display.title === pendingPermission.tool
            ? pendingPermission.tool
            : `${pendingPermission.tool} · ${display.title}`;

    const styles = StyleSheet.create({
        container: {
            borderBottomWidth: variant === 'embedded' ? 1 : 0,
            borderBottomColor: theme.colors.divider,
            marginTop: variant === 'card' ? 8 : 0,
            marginHorizontal: variant === 'card' ? 2 : 0,
            marginBottom: variant === 'card' ? 6 : 0,
            paddingHorizontal: variant === 'card' ? 10 : 8,
            paddingVertical: 10,
            borderRadius: variant === 'card' ? 12 : 0,
            backgroundColor: variant === 'card' ? theme.colors.surface : 'transparent',
            borderWidth: variant === 'card' ? 1 : 0,
            borderColor: theme.colors.divider,
            gap: 8,
        },
        header: {
            flexDirection: 'row',
            alignItems: 'center',
            gap: 8,
        },
        icon: {
            width: 24,
            height: 24,
            borderRadius: 12,
            alignItems: 'center',
            justifyContent: 'center',
            backgroundColor: theme.colors.surfacePressed,
        },
        title: {
            fontSize: variant === 'card' ? 13 : 14,
            fontWeight: variant === 'card' ? '700' : '600',
            color: theme.colors.text,
            ...Typography.default('semiBold'),
        },
        subtitle: {
            fontSize: 12,
            color: theme.colors.textSecondary,
            marginTop: 2,
            ...Typography.default(),
        },
        summary: {
            paddingHorizontal: 10,
            paddingVertical: 8,
            borderRadius: 8,
            backgroundColor: theme.colors.surfacePressed,
            gap: 5,
        },
        detailRow: {
            flexDirection: 'row',
            gap: 8,
            alignItems: 'flex-start',
        },
        detailLabel: {
            width: 58,
            fontSize: 11,
            color: theme.colors.textSecondary,
            ...Typography.default('semiBold'),
        },
        detailValue: {
            flex: 1,
            fontSize: 12,
            color: theme.colors.text,
            ...Typography.default(),
        },
        mono: {
            ...Typography.mono(),
        },
    });

    return (
        <View style={styles.container}>
            <View style={styles.header}>
                <View style={styles.icon}>
                    {commandFromInput(input) ? (
                        <Octicons name="terminal" size={13} color={theme.colors.text} />
                    ) : (
                        <Ionicons name="shield-outline" size={14} color={theme.colors.text} />
                    )}
                </View>
                <View style={{ flex: 1 }}>
                    <Text style={styles.title}>
                        {t('status.permissionRequired')}
                    </Text>
                    <Text style={styles.subtitle} numberOfLines={1}>
                        {subtitle}
                    </Text>
                </View>
            </View>

            {details.length > 0 && (
                <View style={styles.summary}>
                    {details.map((detail) => (
                        <View key={`${detail.label}:${detail.value}`} style={styles.detailRow}>
                            <Text style={styles.detailLabel} numberOfLines={1}>
                                {detail.label}
                            </Text>
                            <Text
                                style={[styles.detailValue, detail.monospace && styles.mono]}
                                numberOfLines={detail.label === 'Command' || detail.label === 'Details' ? 2 : 1}
                            >
                                {detail.value}
                            </Text>
                        </View>
                    ))}
                </View>
            )}

            <PermissionFooter
                permission={{ id: pendingPermission.id, status: 'pending' }}
                sessionId={sessionId}
                toolName={pendingPermission.tool}
                toolInput={input}
                metadata={metadata}
            />
        </View>
    );
}
