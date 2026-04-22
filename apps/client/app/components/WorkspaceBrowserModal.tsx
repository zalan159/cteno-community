import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { View, ScrollView, Pressable, Modal, ActivityIndicator, Image, Platform } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { FileIcon } from '@/components/FileIcon';
import { SimpleSyntaxHighlighter } from '@/components/SimpleSyntaxHighlighter';
import { machineWorkspaceList, machineWorkspaceRead, type WorkspaceEntry, type WorkspaceListResult } from '@/sync/ops';
import { t } from '@/text';

interface WorkspaceBrowserModalProps {
    visible: boolean;
    onClose: () => void;
    machineId: string;
    workspaceRoot?: string;
}

type PreviewState =
    | { kind: 'text'; content: string; language: string | null; bytesRead: number; eof: boolean; size?: number | null }
    | { kind: 'image'; dataUrl: string; bytesRead: number; eof: boolean; size?: number | null }
    | { kind: 'empty'; size?: number | null }
    | { kind: 'binary'; message: string; size?: number | null }
    | { kind: 'unsupported'; message: string; size?: number | null }
    | { kind: 'error'; message: string };

const LIST_LIMIT = 600;
const TEXT_PREVIEW_BYTES = 128 * 1024;
const IMAGE_PREVIEW_BYTES = 2 * 1024 * 1024;

const IMAGE_MIME_MAP: Record<string, string> = {
    png: 'image/png',
    jpg: 'image/jpeg',
    jpeg: 'image/jpeg',
    gif: 'image/gif',
    webp: 'image/webp',
    bmp: 'image/bmp',
    svg: 'image/svg+xml',
    ico: 'image/x-icon',
};

function parentPath(path: string): string | null {
    if (!path || path === '.') return null;
    const normalized = path.replace(/\/+$/, '');
    const idx = normalized.lastIndexOf('/');
    if (idx < 0) return '.';
    if (idx === 0) return '.';
    return normalized.slice(0, idx);
}

function extOf(path: string): string {
    const idx = path.lastIndexOf('.');
    if (idx < 0) return '';
    return path.slice(idx + 1).toLowerCase();
}

function imageMimeForPath(path: string): string | null {
    return IMAGE_MIME_MAP[extOf(path)] ?? null;
}

function isProbablyBinary(text: string): boolean {
    if (!text) return false;
    if (text.includes('\u0000')) return true;
    const sample = text.slice(0, 4096);
    let nonPrintable = 0;
    for (let i = 0; i < sample.length; i++) {
        const code = sample.charCodeAt(i);
        if (code < 9 || (code > 13 && code < 32)) {
            nonPrintable += 1;
        }
    }
    return nonPrintable / Math.max(sample.length, 1) > 0.15;
}

function guessLanguage(path: string): string | null {
    const ext = extOf(path);
    switch (ext) {
        case 'js':
        case 'jsx':
            return 'javascript';
        case 'ts':
        case 'tsx':
            return 'typescript';
        case 'py':
            return 'python';
        case 'html':
        case 'htm':
            return 'html';
        case 'css':
            return 'css';
        case 'json':
            return 'json';
        case 'md':
            return 'markdown';
        case 'xml':
            return 'xml';
        case 'yaml':
        case 'yml':
            return 'yaml';
        case 'sh':
        case 'bash':
        case 'zsh':
            return 'bash';
        case 'sql':
            return 'sql';
        case 'go':
            return 'go';
        case 'rust':
        case 'rs':
            return 'rust';
        case 'java':
            return 'java';
        case 'c':
            return 'c';
        case 'cpp':
        case 'cc':
        case 'cxx':
            return 'cpp';
        case 'php':
            return 'php';
        case 'rb':
            return 'ruby';
        case 'swift':
            return 'swift';
        case 'kt':
            return 'kotlin';
        default:
            return null;
    }
}

function formatBytes(bytes?: number | null): string {
    if (bytes === undefined || bytes === null) return '--';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function displayPath(path: string): string {
    return path === '.' ? '/' : `/${path}`;
}

export const WorkspaceBrowserModal: React.FC<WorkspaceBrowserModalProps> = ({
    visible,
    onClose,
    machineId,
    workspaceRoot,
}) => {
    const { theme } = useUnistyles();
    const [includeHidden, setIncludeHidden] = useState(false);
    const [currentPath, setCurrentPath] = useState('.');
    const [entries, setEntries] = useState<WorkspaceEntry[]>([]);
    const [listLoading, setListLoading] = useState(false);
    const [listError, setListError] = useState<string | null>(null);
    const [selectedFile, setSelectedFile] = useState<WorkspaceEntry | null>(null);
    const [preview, setPreview] = useState<PreviewState | null>(null);
    const [previewLoading, setPreviewLoading] = useState(false);
    const [hasMore, setHasMore] = useState(false);
    const [total, setTotal] = useState(0);

    // Lightweight client-side cache for directory and file preview data.
    const listCacheRef = useRef<Map<string, WorkspaceListResult>>(new Map());
    const previewCacheRef = useRef<Map<string, PreviewState>>(new Map());
    const currentPathRef = useRef('.');
    const loadDirectoryRef = useRef<(path: string, force?: boolean) => Promise<void>>(async () => { });

    const listCacheKey = useCallback((path: string) => `${includeHidden ? 'h1' : 'h0'}:${path}`, [includeHidden]);

    const loadDirectory = useCallback(async (path: string, force = false) => {
        if (!machineId) return;
        const key = listCacheKey(path);

        if (!force) {
            const cached = listCacheRef.current.get(key);
            if (cached) {
                setCurrentPath(cached.path);
                currentPathRef.current = cached.path;
                setEntries(cached.entries);
                setHasMore(cached.hasMore);
                setTotal(cached.total);
                setListError(null);
                return;
            }
        }

        setListLoading(true);
        setListError(null);
        try {
            const result = await machineWorkspaceList(machineId, path, {
                includeHidden,
                limit: LIST_LIMIT,
                workspaceRoot,
            });
            if (!result) {
                setEntries([]);
                setHasMore(false);
                setTotal(0);
                setListError('Failed to load workspace files.');
                return;
            }
            listCacheRef.current.set(key, result);
            setCurrentPath(result.path);
            currentPathRef.current = result.path;
            setEntries(result.entries);
            setHasMore(result.hasMore);
            setTotal(result.total);
        } catch (error) {
            console.error('Failed to load workspace directory:', error);
            setListError('Failed to load workspace files.');
        } finally {
            setListLoading(false);
        }
    }, [includeHidden, listCacheKey, machineId, workspaceRoot]);

    useEffect(() => {
        loadDirectoryRef.current = loadDirectory;
    }, [loadDirectory]);

    const loadPreview = useCallback(async (entry: WorkspaceEntry) => {
        if (!machineId) return;
        const cacheKey = `${entry.path}:${entry.modifiedAt ?? ''}:${entry.size ?? ''}`;
        const cached = previewCacheRef.current.get(cacheKey);
        if (cached) {
            setPreview(cached);
            return;
        }

        setPreviewLoading(true);
        setPreview(null);
        try {
            const mime = imageMimeForPath(entry.path);
            if (mime) {
                if ((entry.size ?? 0) > IMAGE_PREVIEW_BYTES) {
                    const tooLargePreview: PreviewState = {
                        kind: 'unsupported',
                        message: `Image preview is limited to ${formatBytes(IMAGE_PREVIEW_BYTES)}.`,
                        size: entry.size,
                    };
                    previewCacheRef.current.set(cacheKey, tooLargePreview);
                    setPreview(tooLargePreview);
                    return;
                }
                const readResult = await machineWorkspaceRead(machineId, entry.path, {
                    encoding: 'base64',
                    length: IMAGE_PREVIEW_BYTES,
                    workspaceRoot,
                });
                if (!readResult?.data) {
                    const emptyPreview: PreviewState = { kind: 'empty', size: entry.size };
                    previewCacheRef.current.set(cacheKey, emptyPreview);
                    setPreview(emptyPreview);
                    return;
                }
                const imagePreview: PreviewState = {
                    kind: 'image',
                    dataUrl: `data:${mime};base64,${readResult.data}`,
                    bytesRead: readResult.bytesRead ?? 0,
                    eof: readResult.eof ?? true,
                    size: readResult.size ?? entry.size,
                };
                previewCacheRef.current.set(cacheKey, imagePreview);
                setPreview(imagePreview);
                return;
            }

            const readResult = await machineWorkspaceRead(machineId, entry.path, {
                encoding: 'utf8',
                length: TEXT_PREVIEW_BYTES,
                workspaceRoot,
            });
            if (!readResult) {
                const errPreview: PreviewState = { kind: 'error', message: 'Failed to read file.' };
                previewCacheRef.current.set(cacheKey, errPreview);
                setPreview(errPreview);
                return;
            }
            const content = readResult.data ?? '';
            if (!content) {
                const emptyPreview: PreviewState = { kind: 'empty', size: readResult.size ?? entry.size };
                previewCacheRef.current.set(cacheKey, emptyPreview);
                setPreview(emptyPreview);
                return;
            }
            if (isProbablyBinary(content)) {
                const binaryPreview: PreviewState = {
                    kind: 'binary',
                    message: 'Binary file. Preview is not available.',
                    size: readResult.size ?? entry.size,
                };
                previewCacheRef.current.set(cacheKey, binaryPreview);
                setPreview(binaryPreview);
                return;
            }
            const textPreview: PreviewState = {
                kind: 'text',
                content,
                language: guessLanguage(entry.path),
                bytesRead: readResult.bytesRead ?? content.length,
                eof: readResult.eof ?? true,
                size: readResult.size ?? entry.size,
            };
            previewCacheRef.current.set(cacheKey, textPreview);
            setPreview(textPreview);
        } catch (error) {
            console.error('Failed to load file preview:', error);
            const errPreview: PreviewState = { kind: 'error', message: 'Failed to load preview.' };
            setPreview(errPreview);
        } finally {
            setPreviewLoading(false);
        }
    }, [machineId, workspaceRoot]);

    useEffect(() => {
        if (!visible) return;
        setSelectedFile(null);
        setPreview(null);
        setCurrentPath('.');
        currentPathRef.current = '.';
        void loadDirectoryRef.current('.', false);
    }, [visible, machineId]);

    useEffect(() => {
        if (!visible) return;
        setSelectedFile(null);
        setPreview(null);
        void loadDirectoryRef.current(currentPathRef.current, false);
    }, [includeHidden, visible]);

    const pathSegments = useMemo(() => {
        if (currentPath === '.') return [];
        return currentPath.split('/').filter(Boolean);
    }, [currentPath]);

    const handleSelectEntry = useCallback((entry: WorkspaceEntry) => {
        if (entry.type === 'directory') {
            setSelectedFile(null);
            setPreview(null);
            void loadDirectory(entry.path);
            return;
        }
        setSelectedFile(entry);
        void loadPreview(entry);
    }, [loadDirectory, loadPreview]);

    const handleGoParent = useCallback(() => {
        const parent = parentPath(currentPath);
        if (!parent) return;
        setSelectedFile(null);
        setPreview(null);
        void loadDirectory(parent);
    }, [currentPath, loadDirectory]);

    const renderList = () => (
        <View style={{ flex: 1 }}>
            <View style={{
                borderBottomWidth: Platform.select({ ios: 0.33, default: 1 }),
                borderBottomColor: theme.colors.divider,
                backgroundColor: theme.colors.surfaceHigh,
                paddingHorizontal: 12,
                paddingVertical: 10,
            }}>
                <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                        <Pressable
                            onPress={() => {
                                setSelectedFile(null);
                                setPreview(null);
                                void loadDirectory('.', false);
                            }}
                            style={({ pressed }) => ({
                                opacity: pressed ? 0.7 : 1,
                                paddingHorizontal: 8,
                                paddingVertical: 4,
                                borderRadius: 8,
                                backgroundColor: currentPath === '.' ? theme.colors.surfacePressed : 'transparent',
                            })}
                        >
                            <Text style={{ color: theme.colors.textLink, ...Typography.default('semiBold') }}>
                                /
                            </Text>
                        </Pressable>
                        {pathSegments.map((segment, idx) => {
                            const segmentPath = pathSegments.slice(0, idx + 1).join('/');
                            return (
                                <React.Fragment key={segmentPath}>
                                    <Ionicons name="chevron-forward" size={12} color={theme.colors.textSecondary} />
                                    <Pressable
                                        onPress={() => {
                                            setSelectedFile(null);
                                            setPreview(null);
                                            void loadDirectory(segmentPath, false);
                                        }}
                                        style={({ pressed }) => ({
                                            opacity: pressed ? 0.7 : 1,
                                            paddingHorizontal: 8,
                                            paddingVertical: 4,
                                            borderRadius: 8,
                                        })}
                                    >
                                        <Text style={{ color: theme.colors.text, ...Typography.default() }}>
                                            {segment}
                                        </Text>
                                    </Pressable>
                                </React.Fragment>
                            );
                        })}
                    </View>
                </ScrollView>

                <View style={{ marginTop: 8, flexDirection: 'row', alignItems: 'center', gap: 10 }}>
                    <Pressable
                        onPress={handleGoParent}
                        disabled={!parentPath(currentPath)}
                        style={({ pressed }) => ({
                            opacity: !parentPath(currentPath) ? 0.35 : pressed ? 0.7 : 1,
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 4,
                        })}
                    >
                        <Ionicons name="arrow-up-outline" size={16} color={theme.colors.textSecondary} />
                        <Text style={{ color: theme.colors.textSecondary, fontSize: 13, ...Typography.default() }}>
                            Up
                        </Text>
                    </Pressable>

                    <Pressable
                        onPress={() => {
                            setIncludeHidden((prev) => !prev);
                            setSelectedFile(null);
                            setPreview(null);
                        }}
                        style={({ pressed }) => ({
                            opacity: pressed ? 0.7 : 1,
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 4,
                        })}
                    >
                        <Ionicons name={includeHidden ? 'eye' : 'eye-off'} size={16} color={theme.colors.textSecondary} />
                        <Text style={{ color: theme.colors.textSecondary, fontSize: 13, ...Typography.default() }}>
                            Hidden
                        </Text>
                    </Pressable>

                    <Pressable
                        onPress={() => {
                            listCacheRef.current.delete(listCacheKey(currentPath));
                            void loadDirectory(currentPath, true);
                        }}
                        style={({ pressed }) => ({
                            opacity: pressed ? 0.7 : 1,
                            flexDirection: 'row',
                            alignItems: 'center',
                            gap: 4,
                        })}
                    >
                        <Ionicons name="refresh-outline" size={16} color={theme.colors.textSecondary} />
                        <Text style={{ color: theme.colors.textSecondary, fontSize: 13, ...Typography.default() }}>
                            Refresh
                        </Text>
                    </Pressable>
                </View>
            </View>

            {listLoading ? (
                <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}>
                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                </View>
            ) : listError ? (
                <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center', padding: 24 }}>
                    <Ionicons name="warning-outline" size={24} color={theme.colors.textDestructive} />
                    <Text style={{ marginTop: 8, color: theme.colors.textSecondary, textAlign: 'center', ...Typography.default() }}>
                        {listError}
                    </Text>
                </View>
            ) : (
                <ScrollView style={{ flex: 1 }}>
                    {entries.length === 0 ? (
                        <View style={{ padding: 32, alignItems: 'center' }}>
                            <Ionicons name="folder-open-outline" size={34} color={theme.colors.textSecondary} />
                            <Text style={{ marginTop: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                Empty directory
                            </Text>
                        </View>
                    ) : (
                        entries.map((entry) => (
                            <Pressable
                                key={`${entry.type}:${entry.path}`}
                                onPress={() => handleSelectEntry(entry)}
                                style={({ pressed }) => ({
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    paddingHorizontal: 14,
                                    paddingVertical: 12,
                                    borderBottomWidth: Platform.select({ ios: 0.33, default: 1 }),
                                    borderBottomColor: theme.colors.divider,
                                    backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                })}
                            >
                                {entry.type === 'directory' ? (
                                    <Ionicons name="folder-outline" size={20} color="#F5A623" />
                                ) : (
                                    <FileIcon fileName={entry.name} size={20} />
                                )}
                                <View style={{ flex: 1, marginLeft: 10 }}>
                                    <Text numberOfLines={1} style={{ color: theme.colors.text, ...Typography.default() }}>
                                        {entry.name}
                                    </Text>
                                    <Text numberOfLines={1} style={{ marginTop: 2, fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                                        {entry.type === 'directory' ? 'directory' : formatBytes(entry.size)}
                                    </Text>
                                </View>
                                <Ionicons
                                    name={entry.type === 'directory' ? 'chevron-forward' : 'document-text-outline'}
                                    size={16}
                                    color={theme.colors.textSecondary}
                                />
                            </Pressable>
                        ))
                    )}

                    <View style={{ paddingHorizontal: 14, paddingVertical: 10 }}>
                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, ...Typography.default() }}>
                            {hasMore ? `Showing ${entries.length} of ${total} entries` : `${entries.length} entries`}
                        </Text>
                    </View>
                </ScrollView>
            )}
        </View>
    );

    const renderPreview = () => (
        <View style={{ flex: 1 }}>
            <View style={{
                paddingHorizontal: 14,
                paddingVertical: 10,
                borderBottomWidth: Platform.select({ ios: 0.33, default: 1 }),
                borderBottomColor: theme.colors.divider,
                backgroundColor: theme.colors.surfaceHigh,
            }}>
                <Text numberOfLines={1} style={{ color: theme.colors.textSecondary, fontSize: 12, ...Typography.default() }}>
                    {selectedFile ? displayPath(selectedFile.path) : '/'}
                </Text>
                <Text style={{ marginTop: 4, color: theme.colors.textSecondary, fontSize: 12, ...Typography.default() }}>
                    {selectedFile ? formatBytes(selectedFile.size) : '--'}
                </Text>
            </View>

            {previewLoading ? (
                <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}>
                    <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                </View>
            ) : !preview ? (
                <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}>
                    <Text style={{ color: theme.colors.textSecondary, ...Typography.default() }}>
                        Preview unavailable
                    </Text>
                </View>
            ) : preview.kind === 'text' ? (
                <ScrollView style={{ flex: 1 }} contentContainerStyle={{ padding: 14 }}>
                    <SimpleSyntaxHighlighter code={preview.content} language={preview.language} selectable={true} />
                    {!preview.eof && (
                        <Text style={{ marginTop: 12, color: theme.colors.textSecondary, fontSize: 12, ...Typography.default() }}>
                            Partial preview ({formatBytes(preview.bytesRead)} shown).
                        </Text>
                    )}
                </ScrollView>
            ) : preview.kind === 'image' ? (
                <ScrollView style={{ flex: 1 }} contentContainerStyle={{ padding: 14, alignItems: 'center' }}>
                    <Image
                        source={{ uri: preview.dataUrl }}
                        resizeMode="contain"
                        style={{
                            width: '100%',
                            height: 360,
                            borderRadius: 8,
                            backgroundColor: theme.colors.surfaceHigh,
                        }}
                    />
                    {!preview.eof && (
                        <Text style={{ marginTop: 12, color: theme.colors.textSecondary, fontSize: 12, ...Typography.default() }}>
                            Partial image preview ({formatBytes(preview.bytesRead)}).
                        </Text>
                    )}
                </ScrollView>
            ) : (
                <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center', paddingHorizontal: 20 }}>
                    <Ionicons name="document-outline" size={30} color={theme.colors.textSecondary} />
                    <Text style={{ marginTop: 10, color: theme.colors.textSecondary, textAlign: 'center', ...Typography.default() }}>
                        {preview.kind === 'empty'
                            ? 'File is empty.'
                            : preview.kind === 'error'
                                ? preview.message
                                : preview.message}
                    </Text>
                </View>
            )}
        </View>
    );

    return (
        <Modal
            visible={visible}
            animationType="slide"
            presentationStyle="pageSheet"
            onRequestClose={onClose}
        >
            <View style={{ flex: 1, backgroundColor: theme.colors.surface }}>
                <View style={{
                    flexDirection: 'row',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    paddingHorizontal: 16,
                    paddingVertical: 12,
                    borderBottomWidth: Platform.select({ ios: 0.33, default: 1 }),
                    borderBottomColor: theme.colors.divider,
                }}>
                    {selectedFile ? (
                        <Pressable
                            onPress={() => {
                                setSelectedFile(null);
                                setPreview(null);
                            }}
                            style={{ flexDirection: 'row', alignItems: 'center', minWidth: 70 }}
                        >
                            <Ionicons name="chevron-back" size={20} color={theme.colors.textLink} />
                            <Text style={{ color: theme.colors.textLink, ...Typography.default() }}>
                                Back
                            </Text>
                        </Pressable>
                    ) : (
                        <View style={{ minWidth: 70 }} />
                    )}

                    <Text numberOfLines={1} style={{
                        flex: 1,
                        textAlign: 'center',
                        color: theme.colors.text,
                        fontSize: 17,
                        ...Typography.default('semiBold'),
                    }}>
                        {selectedFile ? selectedFile.name : 'Workspace'}
                    </Text>

                    <Pressable onPress={onClose} style={{ minWidth: 70, alignItems: 'flex-end' }}>
                        <Text style={{ color: theme.colors.textLink, ...Typography.default() }}>
                            {t('common.cancel')}
                        </Text>
                    </Pressable>
                </View>

                {selectedFile ? renderPreview() : renderList()}
            </View>
        </Modal>
    );
};
