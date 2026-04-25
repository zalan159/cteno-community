import React, { useState } from 'react';
import { View, Modal, TextInput, Pressable, useWindowDimensions } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { DirectoryPickerModal } from '@/components/DirectoryPickerModal';
import { useIsTablet } from '@/utils/responsive';
import { formatPathRelativeToHome } from '@/utils/sessionUtils';
import { t } from '@/text';

interface NewProjectModalProps {
    visible: boolean;
    machineId?: string;
    homeDir?: string;
    onClose: () => void;
    onCreate: (workdir: string) => Promise<void>;
}

export const NewProjectModal: React.FC<NewProjectModalProps> = ({
    visible,
    machineId,
    homeDir,
    onClose,
    onCreate,
}) => {
    const { theme } = useUnistyles();
    const isTablet = useIsTablet();
    const { width: windowWidth } = useWindowDimensions();
    const [workdir, setWorkdir] = useState('~/');
    const [saving, setSaving] = useState(false);
    const [showDirectoryPicker, setShowDirectoryPicker] = useState(false);
    const canBrowseDirectories = !!machineId && !!homeDir;

    // Match sidebar width calculation from SidebarNavigator/SidebarView
    const sidebarWidth = Math.min(Math.max(Math.floor(windowWidth * 0.3), 250), 360);

    const handleCreate = async () => {
        if (!workdir.trim()) return;
        try {
            setSaving(true);
            await onCreate(workdir.trim());
            setWorkdir('~/');
            onClose();
        } catch (err) {
            console.error('Failed to create project:', err);
        } finally {
            setSaving(false);
        }
    };

    const content = (
        <View style={{
            backgroundColor: theme.colors.surface,
            borderRadius: isTablet ? 14 : 0,
            borderTopLeftRadius: 20,
            borderTopRightRadius: 20,
            padding: 16,
        }}>
            {/* Header */}
            <View style={{
                flexDirection: 'row',
                justifyContent: 'space-between',
                alignItems: 'center',
                paddingBottom: 16,
                borderBottomWidth: 1,
                borderBottomColor: theme.colors.divider,
            }}>
                <Pressable onPress={onClose}>
                    <Text style={{ fontSize: 16, color: theme.colors.textSecondary, ...Typography.default() }}>
                        {t('common.cancel')}
                    </Text>
                </Pressable>
                <Text style={{ fontSize: 17, color: theme.colors.text, ...Typography.default('semiBold') }}>
                    {t('persona.newProject')}
                </Text>
                <Pressable onPress={handleCreate} disabled={!workdir.trim() || saving}>
                    <Text style={{
                        fontSize: 16,
                        color: workdir.trim() && !saving ? theme.colors.textLink : theme.colors.textSecondary,
                        ...Typography.default('semiBold'),
                    }}>
                        {saving ? t('persona.creating') : t('common.create')}
                    </Text>
                </Pressable>
            </View>

            {/* Workdir input */}
            <View style={{ paddingTop: 20, paddingBottom: isTablet ? 20 : 32 }}>
                <Text style={{
                    fontSize: 13,
                    color: theme.colors.textSecondary,
                    marginBottom: 8,
                    ...Typography.default('semiBold'),
                }}>
                    {t('persona.projectPath')}
                </Text>
                <TextInput
                    value={workdir}
                    onChangeText={setWorkdir}
                    placeholder="~/Projects/my-project"
                    placeholderTextColor={theme.colors.textSecondary}
                    autoCapitalize="none"
                    autoCorrect={false}
                    autoFocus
                    style={{
                        backgroundColor: theme.colors.surfaceHigh,
                        borderRadius: 8,
                        padding: 12,
                        fontSize: 16,
                        color: theme.colors.text,
                        ...Typography.default(),
                    }}
                />
                <View style={{ flexDirection: 'row', justifyContent: 'flex-end', marginTop: 10 }}>
                    <Pressable
                        onPress={() => canBrowseDirectories && setShowDirectoryPicker(true)}
                        disabled={!canBrowseDirectories}
                    >
                        <Text style={{
                            fontSize: 13,
                            color: canBrowseDirectories ? theme.colors.textLink : theme.colors.textSecondary,
                            ...Typography.default('semiBold'),
                        }}>
                            {canBrowseDirectories ? '浏览或新建目录' : '目录信息加载中'}
                        </Text>
                    </Pressable>
                </View>
                <Text style={{
                    fontSize: 12,
                    color: theme.colors.textSecondary,
                    marginTop: 8,
                    ...Typography.default(),
                }}>
                    {t('persona.projectSharedMemoryTip')}
                </Text>
            </View>
        </View>
    );

    return (
        <Modal visible={visible} transparent animationType={isTablet ? 'fade' : 'slide'} onRequestClose={onClose}>
            <Pressable
                style={{
                    flex: 1,
                    backgroundColor: 'rgba(0,0,0,0.5)',
                    justifyContent: isTablet ? 'center' : 'flex-end',
                    alignItems: 'center',
                }}
                onPress={onClose}
            >
                <Pressable
                    onPress={(e) => e.stopPropagation()}
                    style={isTablet ? { width: 360 } : { alignSelf: 'stretch' }}
                >
                    {content}
                </Pressable>
            </Pressable>
            <DirectoryPickerModal
                visible={showDirectoryPicker}
                machineId={machineId}
                homeDir={homeDir}
                initialPath={workdir}
                title="选择项目目录"
                onClose={() => setShowDirectoryPicker(false)}
                onSelect={(path) => {
                    setWorkdir(homeDir ? formatPathRelativeToHome(path, homeDir) : path);
                }}
            />
        </Modal>
    );
};
