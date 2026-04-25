import * as React from 'react';
import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ToolViewProps } from './_all';
import { Text } from '@/components/StyledText';
import { RemoteImage } from '@/components/RemoteImage';
import { ImagePreviewModal } from '@/components/ImagePreviewModal';
import { Pressable } from 'react-native';

/**
 * browser_action results:
 * - Text results: "✅ Clicked ...", "✅ Typed ...", "✅ Scrolled ...", etc.
 * - Screenshot results: JSON { type: "browser_screenshot", image_url, images, ... }
 * - Combined: "{action_result}\n\nDOM Changes:\n...\nCurrent URL: ...\n..."
 */

interface ScreenshotResult {
    type: string;
    image_url?: string;
    images?: Array<{ type: string; media_type: string; data: string }>;
}

function parseScreenshotFromResult(result: unknown): ScreenshotResult | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : JSON.stringify(result);
    try {
        const parsed = JSON.parse(raw);
        if (parsed && (parsed.type === 'screenshot' || parsed.type === 'browser_screenshot')
            && (parsed.image_url || parsed.images)) {
            return parsed as ScreenshotResult;
        }
    } catch {
        // Not JSON
    }
    return null;
}

function getImageUri(parsed: ScreenshotResult): string | null {
    if (parsed.image_url) return parsed.image_url;
    if (parsed.images?.[0]?.type === 'base64' && parsed.images[0].data) {
        return `data:${parsed.images[0].media_type};base64,${parsed.images[0].data}`;
    }
    return null;
}

/** Extract the first line (action summary) from the result text */
function getActionSummary(result: unknown): string | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : String(result);
    // Take first meaningful line
    const firstLine = raw.split('\n').find(l => l.trim().length > 0);
    return firstLine?.trim() || null;
}

const ACTION_LABELS: Record<string, string> = {
    click: 'Clicking',
    type: 'Typing',
    type_rich: 'Typing',
    key_press: 'Pressing key',
    scroll: 'Scrolling',
    select: 'Selecting',
    upload: 'Uploading',
    screenshot: 'Taking screenshot',
    evaluate: 'Evaluating JS',
    wait: 'Waiting',
    dismiss_dialogs: 'Dismissing dialogs',
};

export const BrowserActionView = React.memo<ToolViewProps>(({ tool }) => {
    const action = typeof tool.input?.action === 'string' ? tool.input.action : null;
    const [previewVisible, setPreviewVisible] = React.useState(false);

    if (tool.state === 'running') {
        const label = action ? (ACTION_LABELS[action] || action) : 'Executing';
        return (
            <View style={styles.container}>
                <View style={styles.row}>
                    <Text style={styles.runningText}>{label}...</Text>
                </View>
            </View>
        );
    }

    if (tool.state === 'completed') {
        // Check if this is a screenshot action result
        const screenshot = parseScreenshotFromResult(tool.result);
        if (screenshot) {
            const imageUri = getImageUri(screenshot);
            if (imageUri) {
                return (
                    <View style={styles.container}>
                        <Pressable
                            style={({ pressed }) => [styles.imageWrapper, pressed && styles.pressed]}
                            onPress={() => setPreviewVisible(true)}
                        >
                            <RemoteImage
                                uri={imageUri}
                                style={styles.screenshot}
                                resizeMode="contain"
                            />
                        </Pressable>
                        <ImagePreviewModal
                            uri={imageUri}
                            visible={previewVisible}
                            onClose={() => setPreviewVisible(false)}
                        />
                    </View>
                );
            }
        }

        // Text result (click, type, scroll, etc.)
        const summary = getActionSummary(tool.result);
        if (summary) {
            return (
                <View style={styles.container}>
                    <Text style={styles.resultText} numberOfLines={2}>{summary}</Text>
                </View>
            );
        }
    }

    return null;
});

const styles = StyleSheet.create((theme) => ({
    container: {
        paddingVertical: 4,
        paddingBottom: 8,
    },
    row: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
    },
    runningText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
    },
    resultText: {
        fontSize: 13,
        color: theme.colors.textSecondary,
        lineHeight: 18,
    },
    imageWrapper: {
        borderRadius: 8,
        overflow: 'hidden',
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    pressed: {
        opacity: 0.8,
    },
    screenshot: {
        width: '100%',
        aspectRatio: 16 / 10,
        backgroundColor: theme.colors.surface,
    },
}));
