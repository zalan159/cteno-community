import * as React from 'react';
import { View, Pressable, Linking, Platform, Image as RNImage, Modal, ScrollView, ActivityIndicator } from 'react-native';
import { ImagePreviewModal } from '@/components/ImagePreviewModal';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { Text } from '@/components/StyledText';
import { FileIcon } from '@/components/FileIcon';
import { SimpleSyntaxHighlighter } from '@/components/SimpleSyntaxHighlighter';
import { t } from '@/text';
import * as Clipboard from 'expo-clipboard';
import { Modal as AppModal } from '@/modal';
import { useAuth } from '@/auth/AuthContext';
import { getFileDownloadUrl } from '@/sync/apiFiles';
import { RemoteImage } from '@/components/RemoteImage';

type FileCategory = 'image' | 'video' | 'audio' | 'pdf' | 'archive' | 'code' | 'text' | 'file';

function getFileCategoryFromName(name: string): FileCategory {
    const ext = name.split('.').pop()?.toLowerCase() || '';
    if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'ico', 'tiff', 'heic', 'heif', 'avif'].includes(ext)) return 'image';
    if (['mp4', 'mov', 'avi', 'mkv', 'webm', 'flv', 'm4v'].includes(ext)) return 'video';
    if (['mp3', 'wav', 'flac', 'm4a', 'aac', 'ogg', 'wma'].includes(ext)) return 'audio';
    if (ext === 'pdf') return 'pdf';
    if (['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz', 'dmg', 'iso'].includes(ext)) return 'archive';
    if (['js', 'ts', 'tsx', 'jsx', 'py', 'rs', 'go', 'java', 'c', 'cpp', 'h', 'rb', 'php', 'swift', 'kt', 'sh', 'css', 'scss', 'html', 'xml', 'yaml', 'yml', 'toml', 'json', 'sql'].includes(ext)) return 'code';
    if (['md', 'txt', 'log', 'csv', 'rtf', 'doc', 'docx'].includes(ext)) return 'text';
    return 'file';
}

function isPreviewable(category: FileCategory): boolean {
    return category === 'image' || category === 'pdf' || category === 'video' || category === 'text' || category === 'code';
}

function isTextPreviewable(category: FileCategory): boolean {
    return category === 'text' || category === 'code';
}

function getFileExtension(fileName: string): string {
    return fileName.split('.').pop()?.toLowerCase() || '';
}

function isMarkdownFile(fileName: string): boolean {
    return getFileExtension(fileName) === 'md';
}

interface FileDownloadCardProps {
    url?: string;
    fileId?: string;
    filename?: string;
}

export const FileDownloadCard = React.memo((props: FileDownloadCardProps) => {
    const { url: initialUrl, fileId, filename } = props;
    const [downloadUrl, setDownloadUrl] = React.useState<string | null>(initialUrl || null);
    const [isLoadingUrl, setIsLoadingUrl] = React.useState(false);
    const [previewVisible, setPreviewVisible] = React.useState(false);
    const [textContent, setTextContent] = React.useState<string | null>(null);
    const [textLoading, setTextLoading] = React.useState(false);
    const { credentials } = useAuth();

    // Fetch download URL from Happy Server API if fileId is provided
    React.useEffect(() => {
        if (fileId && !downloadUrl && !isLoadingUrl && credentials) {
            setIsLoadingUrl(true);
            getFileDownloadUrl(credentials, fileId)
                .then(data => {
                    setDownloadUrl(data.url);
                })
                .catch(err => {
                    console.error('[FileDownloadCard] Failed to fetch download URL:', err);
                    AppModal.alert(t('common.error'), `Failed to get download URL: ${err.message}`);
                })
                .finally(() => {
                    setIsLoadingUrl(false);
                });
        }
    }, [fileId, downloadUrl, isLoadingUrl, credentials]);

    // Extract filename from URL if not provided
    const displayName = React.useMemo(() => {
        if (filename) return filename;
        if (!downloadUrl) return 'Download';
        try {
            const urlObj = new URL(downloadUrl);
            const pathname = urlObj.pathname;
            const segments = pathname.split('/').filter(Boolean);
            return segments[segments.length - 1] || 'Download';
        } catch {
            return 'Download';
        }
    }, [downloadUrl, filename]);

    const category = React.useMemo(() => getFileCategoryFromName(displayName), [displayName]);
    const canPreview = isPreviewable(category);
    const isImage = category === 'image';
    const isText = isTextPreviewable(category);
    const isMd = isMarkdownFile(displayName);

    const handleDownload = React.useCallback(async () => {
        if (!downloadUrl) {
            AppModal.alert(t('common.error'), 'Download URL not available');
            return;
        }
        try {
            if (Platform.OS === 'web') {
                const link = document.createElement('a');
                link.href = downloadUrl;
                link.target = '_blank';
                link.rel = 'noopener noreferrer';
                if (displayName && displayName !== 'Download') {
                    link.download = displayName;
                }
                document.body.appendChild(link);
                link.click();
                document.body.removeChild(link);
            } else {
                await Linking.openURL(downloadUrl);
            }
        } catch (error) {
            console.error('[FileDownloadCard] Failed to open download URL:', error);
            AppModal.alert(t('common.error'), 'Failed to open download link');
        }
    }, [downloadUrl, displayName]);

    const handleCopyLink = React.useCallback(async () => {
        if (!downloadUrl) {
            AppModal.alert(t('common.error'), 'Download URL not available');
            return;
        }
        try {
            await Clipboard.setStringAsync(downloadUrl);
            AppModal.alert(t('common.success'), 'Download link copied to clipboard');
        } catch (error) {
            console.error('Failed to copy link:', error);
            AppModal.alert(t('common.error'), t('common.copyFailed'));
        }
    }, [downloadUrl]);

    const handlePreview = React.useCallback(async () => {
        if (!downloadUrl) return;
        if (isImage) {
            setPreviewVisible(true);
        } else if (isText) {
            if (textContent !== null) {
                setPreviewVisible(true);
                return;
            }
            setTextLoading(true);
            try {
                const resp = await fetch(downloadUrl);
                const text = await resp.text();
                setTextContent(text);
                setPreviewVisible(true);
            } catch (err) {
                console.error('Failed to fetch text content:', err);
                AppModal.alert(t('common.error'), 'Failed to open download link');
            } finally {
                setTextLoading(false);
            }
        } else {
            // PDF / video: open in browser
            if (Platform.OS === 'web') {
                window.open(downloadUrl, '_blank', 'noopener,noreferrer');
            } else {
                Linking.openURL(downloadUrl);
            }
        }
    }, [isImage, isText, downloadUrl, textContent]);

    return (
        <View style={styles.container}>
            <View style={styles.card}>
                {/* Image thumbnail preview */}
                {isImage && downloadUrl && (
                    <Pressable onPress={() => setPreviewVisible(true)} style={styles.imagePreviewContainer}>
                        <RemoteImage
                            uri={downloadUrl}
                            style={styles.imagePreview}
                            resizeMode="cover"
                        />
                    </Pressable>
                )}

                <View style={styles.header}>
                    <View style={styles.iconContainer}>
                        {isImage ? (
                            <Ionicons name="image-outline" size={28} color="#007AFF" />
                        ) : (
                            <FileIcon fileName={displayName} size={28} />
                        )}
                    </View>
                    <Text style={styles.filename} numberOfLines={2}>{displayName}</Text>
                </View>

                <View style={styles.actions}>
                    {canPreview && downloadUrl && (
                        <Pressable
                            style={({ pressed }) => [
                                styles.actionButton,
                                styles.primaryButton,
                                pressed && styles.actionButtonPressed
                            ]}
                            onPress={handlePreview}
                            disabled={textLoading}
                        >
                            {textLoading ? (
                                <ActivityIndicator size="small" color="#FFFFFF" />
                            ) : (
                                <>
                                    <Ionicons name="eye-outline" size={16} color="#FFFFFF" />
                                    <Text style={styles.primaryButtonText}>{t('common.preview')}</Text>
                                </>
                            )}
                        </Pressable>
                    )}

                    <Pressable
                        style={({ pressed }) => [
                            styles.actionButton,
                            (canPreview && downloadUrl) ? styles.secondaryButton : styles.primaryButton,
                            pressed && styles.actionButtonPressed
                        ]}
                        onPress={handleDownload}
                    >
                        <Ionicons name="download-outline" size={16} color={(canPreview && downloadUrl) ? '#007AFF' : '#FFFFFF'} />
                        <Text style={(canPreview && downloadUrl) ? styles.secondaryButtonText : styles.primaryButtonText}>{t('common.download')}</Text>
                    </Pressable>

                    <Pressable
                        style={({ pressed }) => [
                            styles.iconButton,
                            pressed && styles.actionButtonPressed
                        ]}
                        onPress={handleCopyLink}
                    >
                        <Ionicons name="copy-outline" size={18} color="#007AFF" />
                    </Pressable>
                </View>
            </View>

            {/* Full-screen image preview modal */}
            {isImage && previewVisible && downloadUrl && (
                <ImagePreviewModal
                    uri={downloadUrl}
                    visible={previewVisible}
                    onClose={() => setPreviewVisible(false)}
                />
            )}

            {/* Text/Markdown/Code preview modal */}
            {isText && previewVisible && textContent !== null && (
                <TextPreviewModal
                    content={textContent}
                    fileName={displayName}
                    isMarkdown={isMd}
                    language={isMd ? undefined : getFileExtension(displayName)}
                    visible={previewVisible}
                    onClose={() => setPreviewVisible(false)}
                />
            )}
        </View>
    );
});

// Text/Markdown/Code preview modal
// Lazy-import MarkdownView to avoid circular dependency (MarkdownView → FileDownloadCard → MarkdownView)
const LazyMarkdownView = React.lazy(() =>
    import('@/components/markdown/MarkdownView').then(m => ({ default: m.MarkdownView }))
);

function TextPreviewModal(props: {
    content: string;
    fileName: string;
    isMarkdown: boolean;
    language?: string;
    visible: boolean;
    onClose: () => void;
}) {
    return (
        <Modal visible={props.visible} transparent animationType="slide" onRequestClose={props.onClose}>
            <View style={styles.textModalContainer}>
                <View style={styles.textModalHeader}>
                    <View style={styles.textModalTitleRow}>
                        <FileIcon fileName={props.fileName} size={20} />
                        <Text style={styles.textModalTitle} numberOfLines={1}>{props.fileName}</Text>
                    </View>
                    <Pressable onPress={props.onClose} hitSlop={12}>
                        <View style={styles.modalCloseCircle}>
                            <Text style={styles.textModalCloseText}>✕</Text>
                        </View>
                    </Pressable>
                </View>
                <ScrollView style={styles.textModalBody} contentContainerStyle={styles.textModalBodyContent}>
                    {props.isMarkdown ? (
                        <React.Suspense fallback={<ActivityIndicator size="small" />}>
                            <LazyMarkdownView markdown={props.content} />
                        </React.Suspense>
                    ) : props.language && !['txt', 'log', 'csv', 'rtf'].includes(props.language) ? (
                        <SimpleSyntaxHighlighter code={props.content} language={props.language} selectable />
                    ) : (
                        <Text style={styles.textModalPlainText} selectable>{props.content}</Text>
                    )}
                </ScrollView>
            </View>
        </Modal>
    );
}

const styles = StyleSheet.create((theme) => ({
    container: {
        marginVertical: 8,
    },
    card: {
        backgroundColor: theme.colors.surfaceHighest,
        borderRadius: 12,
        overflow: 'hidden',
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    imagePreviewContainer: {
        width: '100%',
        maxHeight: 200,
        backgroundColor: theme.colors.surfaceHigh,
    },
    imagePreview: {
        width: '100%',
        height: 200,
    },
    header: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 12,
        padding: 12,
        paddingBottom: 8,
    },
    iconContainer: {
        width: 40,
        height: 40,
        borderRadius: 8,
        backgroundColor: theme.colors.surfaceHigh,
        alignItems: 'center',
        justifyContent: 'center',
    },
    filename: {
        flex: 1,
        fontSize: 14,
        fontWeight: '500',
        color: theme.colors.text,
        lineHeight: 20,
    },
    actions: {
        flexDirection: 'row',
        gap: 8,
        paddingHorizontal: 12,
        paddingBottom: 12,
    },
    actionButton: {
        flex: 1,
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 6,
        paddingVertical: 8,
        paddingHorizontal: 12,
        borderRadius: 8,
    },
    iconButton: {
        width: 40,
        height: 40,
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: 8,
        borderWidth: 1,
        borderColor: theme.colors.divider,
        backgroundColor: 'transparent',
    },
    primaryButton: {
        backgroundColor: '#007AFF',
    },
    secondaryButton: {
        backgroundColor: 'transparent',
        borderWidth: 1,
        borderColor: theme.colors.divider,
    },
    actionButtonPressed: {
        opacity: 0.7,
    },
    primaryButtonText: {
        fontSize: 14,
        fontWeight: '500',
        color: '#FFFFFF',
    },
    secondaryButtonText: {
        fontSize: 14,
        fontWeight: '500',
        color: '#007AFF',
    },
    // Shared by TextPreviewModal close button
    modalCloseCircle: {
        width: 36,
        height: 36,
        borderRadius: 18,
        backgroundColor: 'rgba(255,255,255,0.2)',
        alignItems: 'center',
        justifyContent: 'center',
    },
    // Text preview modal
    textModalContainer: {
        flex: 1,
        backgroundColor: theme.colors.surface,
        paddingTop: Platform.OS === 'ios' ? 54 : 24,
    },
    textModalHeader: {
        flexDirection: 'row',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingHorizontal: 16,
        paddingVertical: 12,
        borderBottomWidth: 1,
        borderBottomColor: theme.colors.divider,
    },
    textModalTitleRow: {
        flex: 1,
        flexDirection: 'row',
        alignItems: 'center',
        gap: 8,
        marginRight: 12,
    },
    textModalTitle: {
        fontSize: 16,
        fontWeight: '600',
        color: theme.colors.text,
        flex: 1,
    },
    textModalCloseText: {
        color: theme.colors.text,
        fontSize: 18,
        lineHeight: 20,
    },
    textModalBody: {
        flex: 1,
    },
    textModalBodyContent: {
        padding: 16,
    },
    textModalPlainText: {
        fontSize: 14,
        lineHeight: 22,
        color: theme.colors.text,
        fontFamily: Platform.OS === 'ios' ? 'Menlo' : 'monospace',
    },
}));
