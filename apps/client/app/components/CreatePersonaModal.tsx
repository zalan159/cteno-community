import React, { useState, useRef, useEffect } from 'react';
import { View, ScrollView, Pressable, Modal, TextInput, ActivityIndicator } from 'react-native';
import { Image } from 'expo-image';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import { machineListModels, type ModelOptionDisplay } from '@/sync/ops';
import { MODEL_AVATAR_IMAGES, isModelAvatar, getDefaultAvatarForModelOption } from '@/utils/modelAvatars';

interface CreatePersonaModalProps {
    visible: boolean;
    machineId: string | undefined;
    initialWorkdir?: string;
    onClose: () => void;
    onCreate: (params: {
        name: string;
        description: string;
        modelId?: string;
        avatarId: string;
        workdir: string;
    }) => Promise<void>;
}

const AVATAR_OPTIONS = [
    { id: 'default', icon: 'person-circle-outline', label: 'Default' },
    { id: 'robot', icon: 'hardware-chip-outline', label: 'Robot' },
    { id: 'brain', icon: 'bulb-outline', label: 'Brain' },
    { id: 'star', icon: 'star-outline', label: 'Star' },
    { id: 'rocket', icon: 'rocket-outline', label: 'Rocket' },
    { id: 'code', icon: 'code-slash-outline', label: 'Code' },
    { id: 'book', icon: 'book-outline', label: 'Book' },
    { id: 'search', icon: 'search-outline', label: 'Search' },
];

export const CreatePersonaModal: React.FC<CreatePersonaModalProps> = ({
    visible,
    machineId,
    initialWorkdir,
    onClose,
    onCreate,
}) => {
    const { theme } = useUnistyles();
    const [name, setName] = useState('');
    const [description, setDescription] = useState('');
    const [avatarId, setAvatarId] = useState('default');
    const [workdir, setWorkdir] = useState(initialWorkdir || '~');
    const [saving, setSaving] = useState(false);
    const workdirManuallyEdited = useRef(false);
    const avatarManuallySelected = useRef(false);

    // LLM profile state
    const [profiles, setProfiles] = useState<ModelOptionDisplay[]>([]);
    const [defaultProfileId, setDefaultProfileId] = useState('default');
    const [selectedProfileId, setSelectedProfileId] = useState('default');
    const [loadingProfiles, setLoadingProfiles] = useState(false);

    // Auto-set avatar when profile changes (unless user manually picked one)
    useEffect(() => {
        if (avatarManuallySelected.current) return;
        const selectedProfile = profiles.find((profile) => profile.id === selectedProfileId);
        const modelAvatar = getDefaultAvatarForModelOption({
            modelId: selectedProfile?.id ?? selectedProfileId,
            vendor: selectedProfile?.vendor,
        });
        if (modelAvatar) {
            setAvatarId(modelAvatar);
        }
    }, [profiles, selectedProfileId]);

    // Reset workdir when modal opens with initialWorkdir
    useEffect(() => {
        if (visible && initialWorkdir) {
            setWorkdir(initialWorkdir);
            workdirManuallyEdited.current = true;
        }
    }, [visible, initialWorkdir]);

    // Load profiles when modal opens
    useEffect(() => {
        if (!visible || !machineId) return;
        setLoadingProfiles(true);
        machineListModels(machineId)
            .then(result => {
                setProfiles(result.models || []);
                setDefaultProfileId(result.defaultModelId || 'default');
                // Keep selection if still valid
                const ids = (result.models || []).map(p => p.id);
                if (!ids.includes(selectedProfileId)) {
                    setSelectedProfileId(result.defaultModelId || result.models?.[0]?.id || 'default');
                }
            })
            .catch(err => console.error('Failed to load profiles:', err))
            .finally(() => setLoadingProfiles(false));
    }, [visible, machineId]);

    const handleNameChange = (text: string) => {
        setName(text);
        if (!workdirManuallyEdited.current) {
            const trimmed = text.trim();
            setWorkdir(trimmed ? `~/${trimmed}` : '~');
        }
    };

    const handleWorkdirChange = (text: string) => {
        workdirManuallyEdited.current = true;
        setWorkdir(text);
    };

    const handleCreate = async () => {
        if (!name.trim()) return;
        try {
            setSaving(true);
            await onCreate({
                name: name.trim(),
                description: description.trim(),
                modelId: selectedProfileId,
                avatarId,
                workdir: workdir.trim() || '~',
            });
            // Reset form
            setName('');
            setDescription('');
            setSelectedProfileId(defaultProfileId || 'default');
            setAvatarId('default');
            setWorkdir('~');
            workdirManuallyEdited.current = false;
            avatarManuallySelected.current = false;
            onClose();
        } catch (err) {
            console.error('Failed to create persona:', err);
        } finally {
            setSaving(false);
        }
    };

    return (
        <Modal
            visible={visible}
            transparent
            animationType="slide"
            onRequestClose={onClose}
        >
            <View
                style={{
                    flex: 1,
                    backgroundColor: 'rgba(0,0,0,0.5)',
                    justifyContent: 'flex-end',
                }}
            >
                <View
                    style={{
                        backgroundColor: theme.colors.surface,
                        borderTopLeftRadius: 20,
                        borderTopRightRadius: 20,
                        maxHeight: '80%',
                    }}
                >
                    {/* Header */}
                    <View
                        style={{
                            flexDirection: 'row',
                            justifyContent: 'space-between',
                            alignItems: 'center',
                            padding: 16,
                            borderBottomWidth: 1,
                            borderBottomColor: theme.colors.divider,
                        }}
                    >
                        <Pressable onPress={onClose}>
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: theme.colors.textSecondary,
                                    ...Typography.default(),
                                }}
                            >
                                {t('common.cancel')}
                            </Text>
                        </Pressable>
                        <Text
                            style={{
                                fontSize: 17,
                                color: theme.colors.text,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.newPersona')}
                        </Text>
                        <Pressable
                            onPress={handleCreate}
                            disabled={!name.trim() || saving}
                        >
                            <Text
                                style={{
                                    fontSize: 16,
                                    color: name.trim() && !saving
                                        ? theme.colors.textLink
                                        : theme.colors.textSecondary,
                                    ...Typography.default('semiBold'),
                                }}
                            >
                                {saving ? t('persona.creating') : t('common.create')}
                            </Text>
                        </Pressable>
                    </View>

                    <ScrollView style={{ padding: 16 }}>
                        {/* Avatar selection */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.avatar')}
                        </Text>
                        <View
                            style={{
                                flexDirection: 'row',
                                flexWrap: 'wrap',
                                gap: 8,
                                marginBottom: 20,
                            }}
                        >
                            {/* Model PNG avatars */}
                            {Object.entries(MODEL_AVATAR_IMAGES).map(([id, source]) => (
                                <Pressable
                                    key={id}
                                    onPress={() => { avatarManuallySelected.current = true; setAvatarId(id); }}
                                    style={{
                                        width: 48,
                                        height: 48,
                                        borderRadius: 24,
                                        overflow: 'hidden',
                                        borderWidth: 2,
                                        borderColor: avatarId === id
                                            ? theme.colors.button.primary.background
                                            : 'transparent',
                                    }}
                                >
                                    <Image
                                        source={{ uri: source }}
                                        style={{ width: 44, height: 44, borderRadius: 22 }}
                                        contentFit="cover"
                                    />
                                </Pressable>
                            ))}
                            {/* Icon avatars */}
                            {AVATAR_OPTIONS.map((opt) => (
                                <Pressable
                                    key={opt.id}
                                    onPress={() => { avatarManuallySelected.current = true; setAvatarId(opt.id); }}
                                    style={{
                                        width: 48,
                                        height: 48,
                                        borderRadius: 24,
                                        backgroundColor:
                                            avatarId === opt.id
                                                ? theme.colors.button.primary.background
                                                : theme.colors.surfaceHigh,
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                    }}
                                >
                                    <Ionicons
                                        name={opt.icon as any}
                                        size={24}
                                        color={
                                            avatarId === opt.id
                                                ? theme.colors.button.primary.tint
                                                : theme.colors.text
                                        }
                                    />
                                </Pressable>
                            ))}
                        </View>

                        {/* Name input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.name')}
                        </Text>
                        <TextInput
                            value={name}
                            onChangeText={handleNameChange}
                            placeholder={t('persona.namePlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                ...Typography.default(),
                            }}
                        />

                        {/* Description input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.description')}
                        </Text>
                        <TextInput
                            value={description}
                            onChangeText={setDescription}
                            placeholder={t('persona.descriptionPlaceholder')}
                            placeholderTextColor={theme.colors.textSecondary}
                            multiline
                            numberOfLines={3}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                minHeight: 80,
                                textAlignVertical: 'top',
                                ...Typography.default(),
                            }}
                        />

                        {/* Workdir input */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.workdir')}
                        </Text>
                        <TextInput
                            value={workdir}
                            onChangeText={handleWorkdirChange}
                            placeholder="~/Projects"
                            placeholderTextColor={theme.colors.textSecondary}
                            autoCapitalize="none"
                            autoCorrect={false}
                            style={{
                                backgroundColor: theme.colors.surfaceHigh,
                                borderRadius: 8,
                                padding: 12,
                                fontSize: 16,
                                color: theme.colors.text,
                                marginBottom: 16,
                                ...Typography.default(),
                            }}
                        />

                        {/* LLM Profile selection */}
                        <Text
                            style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold'),
                            }}
                        >
                            {t('persona.model')}
                        </Text>
                        {loadingProfiles ? (
                            <View style={{ padding: 20, alignItems: 'center', marginBottom: 32 }}>
                                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                            </View>
                        ) : profiles.length === 0 ? (
                            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 32, ...Typography.default() }}>
                                无可用模型
                            </Text>
                        ) : (
                            <View style={{ gap: 6, marginBottom: 32 }}>
                                {/* Proxy models (use balance) */}
                                {profiles.some(p => p.isProxy) && (
                                    <>
                                        <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default('semiBold') }}>
                                            内置代理模型（消耗余额）
                                        </Text>
                                        {profiles.filter(p => p.isProxy).map((profile) => (
                                            <Pressable
                                                key={profile.id}
                                                onPress={() => setSelectedProfileId(profile.id)}
                                                style={{
                                                    flexDirection: 'row',
                                                    alignItems: 'center',
                                                    backgroundColor: theme.colors.surfaceHigh,
                                                    borderRadius: 8,
                                                    padding: 10,
                                                    borderWidth: 2,
                                                    borderColor: selectedProfileId === profile.id
                                                        ? theme.colors.button.primary.background
                                                        : 'transparent',
                                                }}
                                            >
                                                {(() => {
                                                    const avatarKey = getDefaultAvatarForModelOption({
                                                        modelId: profile.id,
                                                        vendor: profile.vendor,
                                                    });
                                                    const avatarUri = avatarKey ? MODEL_AVATAR_IMAGES[avatarKey] : null;
                                                    return avatarUri ? (
                                                        <Image
                                                            source={{ uri: avatarUri }}
                                                            style={{ width: 24, height: 24, borderRadius: 12, marginRight: 10 }}
                                                            contentFit="cover"
                                                        />
                                                    ) : (
                                                        <View style={{
                                                            width: 20, height: 20, borderRadius: 10,
                                                            backgroundColor: selectedProfileId === profile.id
                                                                ? theme.colors.button.primary.background
                                                                : theme.colors.success,
                                                            justifyContent: 'center', alignItems: 'center', marginRight: 10,
                                                        }}>
                                                            <Ionicons name="flash-outline" size={12} color="white" />
                                                        </View>
                                                    );
                                                })()}
                                                <View style={{ flex: 1 }}>
                                                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                                                        <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                                            {profile.name}
                                                        </Text>
                                                        {profile.supportsVision && (
                                                            <Ionicons name="image-outline" size={14} color={theme.colors.textSecondary} />
                                                        )}
                                                        {profile.supportsComputerUse && (
                                                            <Ionicons name="desktop-outline" size={14} color={theme.colors.textSecondary} />
                                                        )}
                                                    </View>
                                                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 1, ...Typography.default() }}>
                                                        {profile.chat.model}
                                                    </Text>
                                                </View>
                                                {selectedProfileId === profile.id && (
                                                    <Ionicons name="checkmark-circle" size={20} color={theme.colors.button.primary.background} />
                                                )}
                                            </Pressable>
                                        ))}
                                    </>
                                )}

                                {/* BYOK profiles */}
                                {profiles.some(p => !p.isProxy) && (
                                    <>
                                        <Text style={{
                                            fontSize: 12,
                                            color: theme.colors.textSecondary,
                                            marginBottom: 4,
                                            marginTop: profiles.some(p => p.isProxy) ? 12 : 0,
                                            ...Typography.default('semiBold'),
                                        }}>
                                            自定义 Profile（BYOK）
                                        </Text>
                                        {profiles.filter(p => !p.isProxy).map((profile) => (
                                            <Pressable
                                                key={profile.id}
                                                onPress={() => setSelectedProfileId(profile.id)}
                                                style={{
                                                    flexDirection: 'row',
                                                    alignItems: 'center',
                                                    backgroundColor: theme.colors.surfaceHigh,
                                                    borderRadius: 8,
                                                    padding: 10,
                                                    borderWidth: 2,
                                                    borderColor: selectedProfileId === profile.id
                                                        ? theme.colors.button.primary.background
                                                        : 'transparent',
                                                }}
                                            >
                                                <View style={{
                                                    width: 20,
                                                    height: 20,
                                                    borderRadius: 10,
                                                    backgroundColor: profile.id === defaultProfileId
                                                        ? theme.colors.button.primary.background
                                                        : theme.colors.surfaceHigh,
                                                    justifyContent: 'center',
                                                    alignItems: 'center',
                                                    marginRight: 10,
                                                }}>
                                                    <Ionicons
                                                        name={profile.id === defaultProfileId ? 'star' : 'server-outline'}
                                                        size={12}
                                                        color="white"
                                                    />
                                                </View>
                                                <View style={{ flex: 1 }}>
                                                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                                                        <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                                            {profile.name}
                                                        </Text>
                                                        {profile.supportsVision && (
                                                            <Ionicons name="image-outline" size={14} color={theme.colors.textSecondary} />
                                                        )}
                                                        {profile.supportsComputerUse && (
                                                            <Ionicons name="desktop-outline" size={14} color={theme.colors.textSecondary} />
                                                        )}
                                                    </View>
                                                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 1, ...Typography.default() }}>
                                                        {profile.chat.model} / {profile.compress.model}
                                                    </Text>
                                                </View>
                                                {selectedProfileId === profile.id && (
                                                    <Ionicons name="checkmark-circle" size={20} color={theme.colors.button.primary.background} />
                                                )}
                                            </Pressable>
                                        ))}
                                    </>
                                )}
                            </View>
                        )}
                    </ScrollView>
                </View>
            </View>
        </Modal>
    );
};
