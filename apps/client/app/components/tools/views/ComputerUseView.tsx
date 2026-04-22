import * as React from 'react';
import { View, ActivityIndicator } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolCall } from '@/sync/typesMessage';
import { Metadata } from '@/sync/storageTypes';
import { Text } from '@/components/StyledText';
import { RemoteImage } from '@/components/RemoteImage';

interface ComputerUseResult {
    type: 'screenshot';
    screen_size: [number, number];
    image_url?: string;
    images?: Array<{ type: string; media_type: string; data: string }>;
}

function parseResult(result: unknown): ComputerUseResult | string | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : JSON.stringify(result);
    try {
        const parsed = JSON.parse(raw);
        if (parsed && parsed.type === 'screenshot' && (parsed.image_url || parsed.images)) {
            return parsed as ComputerUseResult;
        }
    } catch {
        // Not JSON - plain text result from click/type/scroll etc.
    }
    return raw;
}

/** Get displayable URI. Priority: image_url > base64 inline */
function getImageUri(parsed: ComputerUseResult): string | null {
    // Primary: OSS URL
    if (parsed.image_url) return parsed.image_url;
    // Fallback: base64 inline (computer_use always provides this)
    if (parsed.images?.[0]?.type === 'base64' && parsed.images[0].data) {
        return `data:${parsed.images[0].media_type};base64,${parsed.images[0].data}`;
    }
    return null;
}

export const ComputerUseView = React.memo((props: { tool: ToolCall, metadata: Metadata | null }) => {
    const { tool } = props;
    const action = tool.input?.action as string | undefined;
    const parsed = parseResult(tool.result);

    // Screenshot result with image
    const imageUri = (parsed && typeof parsed === 'object') ? getImageUri(parsed) : null;
    if (tool.state === 'completed' && parsed && typeof parsed === 'object' && imageUri) {
        const [w, h] = parsed.screen_size || [0, 0];
        return (
            <View style={styles.container}>
                <View style={styles.imageWrapper}>
                    <RemoteImage
                        uri={imageUri}
                        style={styles.screenshot}
                        resizeMode="contain"
                    />
                </View>
                {w > 0 && h > 0 && (
                    <Text style={styles.meta}>{w} x {h}</Text>
                )}
            </View>
        );
    }

    // Running state
    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.runningRow}>
                    <ActivityIndicator size="small" />
                    <Text style={styles.runningText}>
                        {action === 'screenshot' ? 'Taking screenshot...' : `${action || 'Executing'}...`}
                    </Text>
                </View>
            </View>
        );
    }

    // Completed non-screenshot actions (click, type, scroll etc.) - show brief text
    if (tool.state === 'completed' && parsed && typeof parsed === 'string') {
        return (
            <View style={styles.container}>
                <Text style={styles.actionResult} numberOfLines={2}>{parsed}</Text>
            </View>
        );
    }

    return null;
});

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingVertical: 4,
        paddingBottom: 8,
    },
    imageWrapper: {
        borderRadius: 8,
        overflow: 'hidden',
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    screenshot: {
        width: '100%',
        aspectRatio: 16 / 10,
        backgroundColor: theme.colors.surface,
    },
    meta: {
        fontSize: 12,
        color: theme.colors.textSecondary,
        marginTop: 4,
        textAlign: 'right',
    },
    runningRow: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    runningText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
    },
    actionResult: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
}));
