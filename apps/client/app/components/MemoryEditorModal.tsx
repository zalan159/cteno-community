import React, { useState, useEffect, useCallback } from 'react';
import { View, ScrollView, Pressable, Modal, TextInput, ActivityIndicator } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import { machineMemoryListFiles, machineMemoryRead, machineMemoryWrite, machineMemoryDelete } from '@/sync/ops';

interface MemoryEditorModalProps {
    visible: boolean;
    onClose: () => void;
    machineId: string;
    ownerId?: string;
}

/** Parse tagged file path from backend: "[private:Name] path" or "[global] path" */
function parseTaggedPath(tagged: string): { scope: 'private' | 'global'; path: string } {
    const privateMatch = tagged.match(/^\[private(?::[^\]]+)?\]\s+(.+)/);
    if (privateMatch) return { scope: 'private', path: privateMatch[1] };
    if (tagged.startsWith('[global] ')) return { scope: 'global', path: tagged.slice(9) };
    return { scope: 'global', path: tagged };
}

export const MemoryEditorModal: React.FC<MemoryEditorModalProps> = ({
    visible,
    onClose,
    machineId,
    ownerId,
}) => {
    const { theme } = useUnistyles();
    const [files, setFiles] = useState<string[]>([]);
    const [loading, setLoading] = useState(false);
    const [selectedFile, setSelectedFile] = useState<string | null>(null);
    const [selectedScope, setSelectedScope] = useState<'private' | 'global'>('global');
    const [selectedCleanPath, setSelectedCleanPath] = useState<string>('');
    const [content, setContent] = useState('');
    const [saving, setSaving] = useState(false);
    const [dirty, setDirty] = useState(false);
    const [deleting, setDeleting] = useState(false);
    const [confirmDeletePath, setConfirmDeletePath] = useState<string | null>(null);

    const loadFiles = useCallback(async () => {
        if (!machineId) return;
        setLoading(true);
        try {
            const result = await machineMemoryListFiles(machineId, ownerId);
            setFiles(result);
        } finally {
            setLoading(false);
        }
    }, [machineId, ownerId]);

    useEffect(() => {
        if (visible) {
            loadFiles();
            setSelectedFile(null);
            setContent('');
            setDirty(false);
        }
    }, [visible, loadFiles]);

    const handleSelectFile = useCallback(async (taggedPath: string) => {
        const { scope, path } = parseTaggedPath(taggedPath);
        setSelectedFile(taggedPath);
        setSelectedScope(scope);
        setSelectedCleanPath(path);
        setLoading(true);
        try {
            const result = await machineMemoryRead(machineId, path, ownerId, scope);
            setContent(result ?? '');
            setDirty(false);
        } finally {
            setLoading(false);
        }
    }, [machineId, ownerId]);

    const handleSave = useCallback(async () => {
        if (!selectedCleanPath) return;
        setSaving(true);
        try {
            await machineMemoryWrite(machineId, selectedCleanPath, content, ownerId, selectedScope);
            setDirty(false);
        } finally {
            setSaving(false);
        }
    }, [machineId, selectedCleanPath, content, ownerId, selectedScope]);

    const handleBack = useCallback(() => {
        setSelectedFile(null);
        setContent('');
        setDirty(false);
    }, []);

    const handleClose = useCallback(() => {
        onClose();
    }, [onClose]);

    const handleDeleteFile = useCallback(async (taggedPath: string) => {
        const { scope, path } = parseTaggedPath(taggedPath);
        setDeleting(true);
        try {
            const ok = await machineMemoryDelete(machineId, path, ownerId, scope);
            if (ok) {
                // If we're viewing the deleted file, go back to list
                if (selectedFile === taggedPath) {
                    setSelectedFile(null);
                    setContent('');
                    setDirty(false);
                }
                await loadFiles();
            }
        } finally {
            setDeleting(false);
            setConfirmDeletePath(null);
        }
    }, [machineId, ownerId, selectedFile, loadFiles]);

    return (
        <Modal
            visible={visible}
            animationType="slide"
            presentationStyle="pageSheet"
            onRequestClose={handleClose}
        >
            <View style={{
                flex: 1,
                backgroundColor: theme.colors.surface,
            }}>
                {/* Header */}
                <View style={{
                    flexDirection: 'row',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    paddingHorizontal: 16,
                    paddingVertical: 12,
                    borderBottomWidth: 0.5,
                    borderBottomColor: theme.colors.divider,
                }}>
                    {selectedFile ? (
                        <Pressable onPress={handleBack} style={{ flexDirection: 'row', alignItems: 'center' }}>
                            <Ionicons name="chevron-back" size={20} color={theme.colors.textLink} />
                            <Text style={{
                                fontSize: 16,
                                color: theme.colors.textLink,
                                ...Typography.default(),
                            }}>
                                {t('memory.backToList')}
                            </Text>
                        </Pressable>
                    ) : (
                        <View style={{ width: 60 }} />
                    )}

                    <Text style={{
                        fontSize: 17,
                        ...Typography.default('semiBold'),
                        color: theme.colors.text,
                        flex: 1,
                        textAlign: 'center',
                    }}>
                        {selectedFile ?? t('memory.title')}
                    </Text>

                    {selectedFile && dirty ? (
                        <Pressable
                            onPress={handleSave}
                            disabled={saving}
                            style={({ pressed }) => ({
                                paddingHorizontal: 12,
                                paddingVertical: 6,
                                borderRadius: 8,
                                backgroundColor: pressed
                                    ? theme.colors.surfacePressed
                                    : theme.colors.button.primary.background,
                            })}
                        >
                            {saving ? (
                                <ActivityIndicator size="small" color={theme.colors.button.primary.tint} />
                            ) : (
                                <Text style={{
                                    fontSize: 15,
                                    color: theme.colors.button.primary.tint,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {t('common.save')}
                                </Text>
                            )}
                        </Pressable>
                    ) : (
                        <Pressable onPress={handleClose}>
                            <Text style={{
                                fontSize: 16,
                                color: theme.colors.textLink,
                                ...Typography.default(),
                            }}>
                                {t('common.cancel')}
                            </Text>
                        </Pressable>
                    )}
                </View>

                {loading && !selectedFile ? (
                    <View style={{ padding: 40, alignItems: 'center' }}>
                        <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                    </View>
                ) : selectedFile ? (
                    /* Editor view */
                    <View style={{ flex: 1 }}>
                        {loading ? (
                            <View style={{ padding: 40, alignItems: 'center' }}>
                                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                            </View>
                        ) : (
                            <TextInput
                                value={content}
                                onChangeText={(text) => {
                                    setContent(text);
                                    setDirty(true);
                                }}
                                multiline
                                style={{
                                    flex: 1,
                                    padding: 16,
                                    fontSize: 14,
                                    lineHeight: 20,
                                    color: theme.colors.text,
                                    fontFamily: 'monospace',
                                    textAlignVertical: 'top',
                                }}
                                autoCapitalize="none"
                                autoCorrect={false}
                            />
                        )}
                    </View>
                ) : (
                    /* File list view */
                    <ScrollView style={{ flex: 1 }}>
                        {files.length === 0 ? (
                            <View style={{ padding: 40, alignItems: 'center' }}>
                                <Ionicons name="document-text-outline" size={40} color={theme.colors.textSecondary} />
                                <Text style={{
                                    marginTop: 12,
                                    fontSize: 15,
                                    color: theme.colors.textSecondary,
                                    ...Typography.default(),
                                }}>
                                    {t('memory.empty')}
                                </Text>
                            </View>
                        ) : (
                            files.map((filePath) => (
                                <View
                                    key={filePath}
                                    style={{
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        borderBottomWidth: 0.5,
                                        borderBottomColor: theme.colors.divider,
                                    }}
                                >
                                    {confirmDeletePath === filePath ? (
                                        <View style={{
                                            flex: 1,
                                            flexDirection: 'row',
                                            alignItems: 'center',
                                            justifyContent: 'flex-end',
                                            paddingHorizontal: 16,
                                            paddingVertical: 10,
                                            gap: 12,
                                        }}>
                                            <Text style={{
                                                flex: 1,
                                                fontSize: 14,
                                                color: theme.colors.textSecondary,
                                                ...Typography.default(),
                                            }}>
                                                {t('memory.deleteConfirm')}
                                            </Text>
                                            <Pressable
                                                onPress={() => setConfirmDeletePath(null)}
                                                style={{
                                                    paddingHorizontal: 12,
                                                    paddingVertical: 6,
                                                    borderRadius: 6,
                                                    backgroundColor: theme.colors.surfacePressed,
                                                }}
                                            >
                                                <Text style={{
                                                    fontSize: 14,
                                                    color: theme.colors.text,
                                                    ...Typography.default(),
                                                }}>
                                                    {t('common.cancel')}
                                                </Text>
                                            </Pressable>
                                            <Pressable
                                                onPress={() => handleDeleteFile(filePath)}
                                                disabled={deleting}
                                                style={{
                                                    paddingHorizontal: 12,
                                                    paddingVertical: 6,
                                                    borderRadius: 6,
                                                    backgroundColor: theme.colors.textDestructive,
                                                }}
                                            >
                                                {deleting ? (
                                                    <ActivityIndicator size="small" color="#fff" />
                                                ) : (
                                                    <Text style={{
                                                        fontSize: 14,
                                                        color: '#fff',
                                                        ...Typography.default('semiBold'),
                                                    }}>
                                                        {t('common.delete')}
                                                    </Text>
                                                )}
                                            </Pressable>
                                        </View>
                                    ) : (
                                        <>
                                            <Pressable
                                                onPress={() => handleSelectFile(filePath)}
                                                style={({ pressed }) => ({
                                                    flex: 1,
                                                    flexDirection: 'row',
                                                    alignItems: 'center',
                                                    paddingLeft: 16,
                                                    paddingVertical: 14,
                                                    backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                                })}
                                            >
                                                <Ionicons
                                                    name="document-text-outline"
                                                    size={20}
                                                    color={theme.colors.textSecondary}
                                                />
                                                <Text style={{
                                                    marginLeft: 12,
                                                    fontSize: 15,
                                                    color: theme.colors.text,
                                                    flex: 1,
                                                    ...Typography.default(),
                                                }}>
                                                    {filePath}
                                                </Text>
                                                <Ionicons
                                                    name="chevron-forward"
                                                    size={16}
                                                    color={theme.colors.textSecondary}
                                                />
                                            </Pressable>
                                            <Pressable
                                                onPress={() => setConfirmDeletePath(filePath)}
                                                style={({ pressed }) => ({
                                                    paddingHorizontal: 14,
                                                    paddingVertical: 14,
                                                    opacity: pressed ? 0.5 : 1,
                                                })}
                                            >
                                                <Ionicons
                                                    name="trash-outline"
                                                    size={18}
                                                    color={theme.colors.textDestructive}
                                                />
                                            </Pressable>
                                        </>
                                    )}
                                </View>
                            ))
                        )}
                    </ScrollView>
                )}
            </View>
        </Modal>
    );
};
