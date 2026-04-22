import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { ActivityIndicator, Modal, Pressable, ScrollView, TextInput, View } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { machineWorkspaceList, type WorkspaceEntry } from '@/sync/ops';
import { resolveAbsolutePath } from '@/utils/pathUtils';
import { formatPathRelativeToHome } from '@/utils/sessionUtils';
import { t } from '@/text';

interface DirectoryPickerModalProps {
    visible: boolean;
    machineId?: string;
    homeDir?: string;
    initialPath?: string;
    title?: string;
    onClose: () => void;
    onSelect: (path: string) => void;
}

function parentRelativePath(path: string): string | null {
    if (!path || path === '.') return null;
    const trimmed = path.replace(/\/+$/, '');
    const idx = trimmed.lastIndexOf('/');
    if (idx < 0) return '.';
    if (idx === 0) return '.';
    return trimmed.slice(0, idx);
}

function joinRelativePath(base: string, child: string): string {
    const cleaned = child.replace(/^\/+|\/+$/g, '');
    if (!cleaned) return base;
    return base === '.' ? cleaned : `${base}/${cleaned}`;
}

function separatorFor(homeDir?: string): '/' | '\\' {
    if (!homeDir) return '/';
    return homeDir.lastIndexOf('\\') > homeDir.lastIndexOf('/') ? '\\' : '/';
}

function joinAbsolutePath(base: string, child: string): string {
    const separator = separatorFor(base);
    const normalizedBase = base.endsWith('/') || base.endsWith('\\') ? base.slice(0, -1) : base;
    return `${normalizedBase}${separator}${child}`;
}

function normalizeForCompare(path: string): string {
    return path.replace(/\\/g, '/').replace(/\/+$/, '');
}

function relativeToHome(path: string, homeDir?: string): string {
    if (!homeDir) return '.';
    const normalizedHome = normalizeForCompare(homeDir);
    const normalizedPath = normalizeForCompare(path);

    if (normalizedPath === normalizedHome) return '.';
    if (normalizedPath.startsWith(`${normalizedHome}/`)) {
        return normalizedPath.slice(normalizedHome.length + 1);
    }
    return '.';
}

function isValidNewDirectoryName(value: string): boolean {
    const trimmed = value.trim();
    return !!trimmed && !trimmed.includes('/') && !trimmed.includes('\\') && trimmed !== '.' && trimmed !== '..';
}

export const DirectoryPickerModal: React.FC<DirectoryPickerModalProps> = ({
    visible,
    machineId,
    homeDir,
    initialPath,
    title = '选择工作目录',
    onClose,
    onSelect,
}) => {
    const { theme } = useUnistyles();
    const [currentPath, setCurrentPath] = useState('.');
    const [directories, setDirectories] = useState<WorkspaceEntry[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [newDirectoryName, setNewDirectoryName] = useState('');

    const currentAbsolutePath = useMemo(() => {
        if (!homeDir) return initialPath || '~';
        return currentPath === '.' ? homeDir : joinAbsolutePath(homeDir, currentPath);
    }, [currentPath, homeDir, initialPath]);

    const currentDisplayPath = useMemo(() => {
        return homeDir
            ? formatPathRelativeToHome(currentAbsolutePath, homeDir)
            : currentAbsolutePath;
    }, [currentAbsolutePath, homeDir]);

    const loadDirectory = useCallback(async (relativePath: string) => {
        if (!visible || !machineId || !homeDir) return;
        setLoading(true);
        setError(null);

        const candidates: string[] = [];
        let nextCandidate: string | null = relativePath || '.';
        while (nextCandidate) {
            candidates.push(nextCandidate);
            nextCandidate = parentRelativePath(nextCandidate);
        }
        if (!candidates.includes('.')) {
            candidates.push('.');
        }

        try {
            for (const candidate of candidates) {
                const result = await machineWorkspaceList(machineId, candidate, {
                    workspaceRoot: homeDir,
                    limit: 500,
                });
                if (!result) continue;

                setCurrentPath(result.path || candidate);
                setDirectories((result.entries || []).filter((entry) => entry.type === 'directory'));
                return;
            }

            setCurrentPath('.');
            setDirectories([]);
            setError('目录加载失败');
        } catch (loadError) {
            console.error('Failed to browse directories:', loadError);
            setCurrentPath('.');
            setDirectories([]);
            setError('目录加载失败');
        } finally {
            setLoading(false);
        }
    }, [visible, machineId, homeDir]);

    useEffect(() => {
        if (!visible) return;
        setNewDirectoryName('');

        if (!homeDir) {
            setCurrentPath('.');
            setDirectories([]);
            return;
        }

        const absolute = resolveAbsolutePath(initialPath || '~', homeDir);
        loadDirectory(relativeToHome(absolute, homeDir));
    }, [visible, homeDir, initialPath, loadDirectory]);

    const handleChooseCurrent = useCallback(() => {
        onSelect(currentAbsolutePath);
        onClose();
    }, [currentAbsolutePath, onClose, onSelect]);

    const handleCreateSubdirectory = useCallback(() => {
        const trimmed = newDirectoryName.trim();
        if (!isValidNewDirectoryName(trimmed)) return;
        const newPath = joinAbsolutePath(currentAbsolutePath, trimmed);
        onSelect(newPath);
        setNewDirectoryName('');
        onClose();
    }, [currentAbsolutePath, newDirectoryName, onClose, onSelect]);

    return (
        <Modal visible={visible} transparent animationType="slide" onRequestClose={onClose}>
            <Pressable
                style={{
                    flex: 1,
                    backgroundColor: 'rgba(0,0,0,0.5)',
                    justifyContent: 'flex-end',
                }}
                onPress={onClose}
            >
                <Pressable
                    onPress={(event) => event.stopPropagation()}
                    style={{
                        backgroundColor: theme.colors.surface,
                        borderTopLeftRadius: 20,
                        borderTopRightRadius: 20,
                        padding: 16,
                        gap: 16,
                        maxHeight: '82%',
                    }}
                >
                    <View
                        style={{
                            flexDirection: 'row',
                            justifyContent: 'space-between',
                            alignItems: 'center',
                            borderBottomWidth: 1,
                            borderBottomColor: theme.colors.divider,
                            paddingBottom: 16,
                        }}
                    >
                        <Pressable onPress={onClose}>
                            <Text style={{ fontSize: 16, color: theme.colors.textSecondary, ...Typography.default() }}>
                                {t('common.cancel')}
                            </Text>
                        </Pressable>
                        <Text style={{ fontSize: 17, color: theme.colors.text, ...Typography.default('semiBold') }}>
                            {title}
                        </Text>
                        <Pressable onPress={handleChooseCurrent} disabled={!machineId || !homeDir || loading}>
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: machineId && homeDir && !loading ? theme.colors.textLink : theme.colors.textSecondary,
                                    ...Typography.default('semiBold'),
                                }}
                            >
                                使用当前目录
                            </Text>
                        </Pressable>
                    </View>

                    <View style={{ gap: 6 }}>
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                            当前目录
                        </Text>
                        <View
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 10,
                                padding: 12,
                            }}
                        >
                            <Text style={{ fontSize: 15, color: theme.colors.text, ...Typography.default() }}>
                                {currentDisplayPath}
                            </Text>
                        </View>
                    </View>

                    <View style={{ gap: 8 }}>
                        <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                            新建子目录
                        </Text>
                        <View style={{ flexDirection: 'row', gap: 8 }}>
                            <TextInput
                                value={newDirectoryName}
                                onChangeText={setNewDirectoryName}
                                placeholder="例如：my-project"
                                placeholderTextColor={theme.colors.textSecondary}
                                autoCapitalize="none"
                                autoCorrect={false}
                                style={{
                                    flex: 1,
                                    backgroundColor: theme.colors.surfaceHigh,
                                    borderRadius: 10,
                                    paddingHorizontal: 12,
                                    paddingVertical: 12,
                                    color: theme.colors.text,
                                    fontSize: 16,
                                    ...Typography.default(),
                                }}
                            />
                            <Pressable
                                onPress={handleCreateSubdirectory}
                                disabled={!isValidNewDirectoryName(newDirectoryName)}
                                style={{
                                    borderRadius: 10,
                                    paddingHorizontal: 14,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: isValidNewDirectoryName(newDirectoryName)
                                        ? theme.colors.button.primary.background
                                        : theme.colors.surfaceHigh,
                                }}
                            >
                                <Text
                                    style={{
                                        color: isValidNewDirectoryName(newDirectoryName)
                                            ? theme.colors.button.primary.tint
                                            : theme.colors.textSecondary,
                                        ...Typography.default('semiBold'),
                                    }}
                                >
                                    新建
                                </Text>
                            </Pressable>
                        </View>
                    </View>

                    <View style={{ flex: 1, minHeight: 260 }}>
                        <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10 }}>
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default('semiBold') }}>
                                浏览目录
                            </Text>
                            {currentPath !== '.' && (
                                <Pressable
                                    onPress={() => loadDirectory(parentRelativePath(currentPath) || '.')}
                                    style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}
                                >
                                    <Ionicons name="arrow-up-outline" size={16} color={theme.colors.textSecondary} />
                                    <Text style={{ fontSize: 13, color: theme.colors.textSecondary, ...Typography.default() }}>
                                        上一级
                                    </Text>
                                </Pressable>
                            )}
                        </View>

                        {loading ? (
                            <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}>
                                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                            </View>
                        ) : error ? (
                            <View
                                style={{
                                    flex: 1,
                                    borderRadius: 12,
                                    backgroundColor: theme.colors.surfaceHigh,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    padding: 16,
                                }}
                            >
                                <Text style={{ color: theme.colors.textSecondary, textAlign: 'center', ...Typography.default() }}>
                                    {error}
                                </Text>
                            </View>
                        ) : (
                            <ScrollView
                                style={{
                                    flex: 1,
                                    borderRadius: 12,
                                    backgroundColor: theme.colors.surfaceHigh,
                                }}
                                contentContainerStyle={{ paddingVertical: 6 }}
                            >
                                {directories.length === 0 ? (
                                    <View style={{ padding: 16 }}>
                                        <Text style={{ color: theme.colors.textSecondary, ...Typography.default() }}>
                                            当前目录下没有可继续进入的子目录
                                        </Text>
                                    </View>
                                ) : (
                                    directories.map((entry) => (
                                        <Pressable
                                            key={entry.path}
                                            onPress={() => loadDirectory(entry.path)}
                                            style={({ pressed }) => ({
                                                flexDirection: 'row',
                                                alignItems: 'center',
                                                justifyContent: 'space-between',
                                                paddingHorizontal: 14,
                                                paddingVertical: 12,
                                                backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                            })}
                                        >
                                            <View style={{ flexDirection: 'row', alignItems: 'center', flex: 1, gap: 10 }}>
                                                <Ionicons name="folder-outline" size={18} color={theme.colors.textSecondary} />
                                                <Text style={{ flex: 1, color: theme.colors.text, ...Typography.default() }}>
                                                    {entry.name}
                                                </Text>
                                            </View>
                                            <Ionicons name="chevron-forward" size={16} color={theme.colors.textSecondary} />
                                        </Pressable>
                                    ))
                                )}
                            </ScrollView>
                        )}
                    </View>
                </Pressable>
            </Pressable>
        </Modal>
    );
};
