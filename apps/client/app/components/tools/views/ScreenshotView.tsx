import * as React from 'react';
import { View, ActivityIndicator, Pressable, Linking, Platform } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { ToolCall } from '@/sync/typesMessage';
import { Metadata } from '@/sync/storageTypes';
import { Text } from '@/components/StyledText';
import { ImagePreviewModal } from '@/components/ImagePreviewModal';
import { RemoteImage } from '@/components/RemoteImage';

interface ScreenshotResult {
    type: 'screenshot' | 'browser_screenshot';
    screen_size?: [number, number];
    image_url?: string;
    image_path?: string;
    images?: Array<{ type: string; media_type: string; data: string }>;
}

function parseResult(result: unknown): ScreenshotResult | null {
    if (!result) return null;
    const raw = typeof result === 'string' ? result : JSON.stringify(result);
    try {
        const parsed = JSON.parse(raw);
        if (parsed && (parsed.type === 'screenshot' || parsed.type === 'browser_screenshot')
            && (parsed.image_url || parsed.images || parsed.image_path)) {
            return parsed as ScreenshotResult;
        }
    } catch {
        // Not valid JSON
    }
    return null;
}

/** Get displayable URI from screenshot result. Priority: image_url > base64 inline */
function getImageUri(parsed: ScreenshotResult): string | null {
    // Primary: OSS URL (efficient, no base64 in session history)
    if (parsed.image_url) {
        return parsed.image_url;
    }
    // Fallback: base64 inline (computer_use, legacy)
    if (parsed.images?.[0]?.type === 'base64' && parsed.images[0].data) {
        return `data:${parsed.images[0].media_type};base64,${parsed.images[0].data}`;
    }
    return null;
}

export const ScreenshotView = React.memo((props: { tool: ToolCall, metadata: Metadata | null }) => {
    const { tool } = props;
    const parsed = parseResult(tool.result);
    const [previewVisible, setPreviewVisible] = React.useState(false);

    // Completed: show the screenshot image
    const imageUri = parsed ? getImageUri(parsed) : null;
    if (tool.state === 'completed' && parsed && imageUri) {
        const [w, h] = parsed.screen_size || [0, 0];
        return (
            <View style={styles.container}>
                <Pressable
                    style={({ pressed }) => [styles.imageWrapper, pressed && styles.imagePressed]}
                    onPress={() => setPreviewVisible(true)}
                >
                    <RemoteImage
                        uri={imageUri}
                        style={styles.screenshot}
                        resizeMode="contain"
                    />
                </Pressable>
                <View style={styles.footer}>
                    {w > 0 && h > 0 && (
                        <Text style={styles.meta}>{w} x {h}</Text>
                    )}
                    <Pressable
                        style={({ pressed }) => [styles.downloadButton, pressed && styles.downloadButtonPressed]}
                        onPress={() => {
                            if (Platform.OS === 'web') {
                                const link = document.createElement('a');
                                link.href = imageUri;
                                link.target = '_blank';
                                link.rel = 'noopener noreferrer';
                                link.download = 'screenshot.png';
                                document.body.appendChild(link);
                                link.click();
                                document.body.removeChild(link);
                            } else {
                                Linking.openURL(imageUri).catch(() => {});
                            }
                        }}
                    >
                        <Ionicons name="download-outline" size={14} color="#007AFF" />
                        <Text style={styles.downloadText}>Download</Text>
                    </Pressable>
                </View>

                <ImagePreviewModal
                    uri={imageUri}
                    visible={previewVisible}
                    onClose={() => setPreviewVisible(false)}
                />
            </View>
        );
    }

    // Running state
    if (tool.state === 'running') {
        return (
            <View style={styles.container}>
                <View style={styles.runningRow}>
                    <ActivityIndicator size="small" />
                    <Text style={styles.runningText}>Taking screenshot...</Text>
                </View>
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
    imagePressed: {
        opacity: 0.8,
    },
    screenshot: {
        width: '100%',
        aspectRatio: 16 / 10,
        backgroundColor: theme.colors.surface,
    },
    footer: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        marginTop: 4,
    },
    meta: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
    downloadButton: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 4,
        paddingVertical: 2,
        paddingHorizontal: 6,
        borderRadius: 4,
    },
    downloadButtonPressed: {
        opacity: 0.6,
    },
    downloadText: {
        fontSize: 12,
        color: '#007AFF',
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
}));
