import React from 'react';
import { View, Pressable, ScrollView, Alert, TextInput, ActivityIndicator } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { useAllMachines } from '@/sync/storage';
import { StyleSheet } from 'react-native-unistyles';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { t } from '@/text';
import { layout } from '@/components/layout';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { useWindowDimensions } from 'react-native';
import { randomUUID } from 'expo-crypto';
import { isMachineOnline } from '@/utils/machineUtils';
import { useLocalSearchParams } from 'expo-router';
import {
    machineListProfiles,
    machineSaveProfile,
    machineSaveCodingPlanProfiles,
    machineDeleteProfile,
    machineExportProfiles,
    type ModelOptionDisplay,
    type LlmProfileInput,
    type LlmEndpointInput,
    type LlmProfileFull,
} from '@/sync/ops';
import {
    CODING_PLAN_PRESETS,
    buildCodingPlanProfiles,
    getCodingPlanProvider,
    type CodingPlanProviderId,
    type CodingPlanRegion,
} from '@/sync/codingPlanPresets';
import { Text } from '@/components/StyledText';
import { Image } from 'expo-image';
import { getDefaultAvatarForModelId, MODEL_AVATAR_IMAGES } from '@/utils/modelAvatars';
import { Modal } from '@/modal';

function ProfileManager() {
    const { theme } = useUnistyles();
    const machines = useAllMachines();
    const safeArea = useSafeAreaInsets();
    const screenWidth = useWindowDimensions().width;
    const { machineId: routeMachineId } = useLocalSearchParams<{ machineId?: string }>();

    const [profiles, setProfiles] = React.useState<ModelOptionDisplay[]>([]);
    const [defaultProfileId, setDefaultProfileId] = React.useState<string>('default');
    const [loading, setLoading] = React.useState(true);
    const [editingProfile, setEditingProfile] = React.useState<LlmProfileInput | null>(null);
    const [isEditingExisting, setIsEditingExisting] = React.useState(false);
    const [showEditForm, setShowEditForm] = React.useState(false);

    // Coding Plan preset modal state
    const [showCodingPlanModal, setShowCodingPlanModal] = React.useState(false);
    const [codingPlanProviderId, setCodingPlanProviderId] = React.useState<CodingPlanProviderId>('bailian');
    const [codingPlanRegion, setCodingPlanRegion] = React.useState<CodingPlanRegion>('intl');
    const [codingPlanApiKey, setCodingPlanApiKey] = React.useState('');
    const [codingPlanSaving, setCodingPlanSaving] = React.useState(false);

    // Import modal state
    const [showImportModal, setShowImportModal] = React.useState(false);
    const [importSourceId, setImportSourceId] = React.useState<string | null>(null);
    const [importProfiles, setImportProfiles] = React.useState<LlmProfileFull[]>([]);
    const [importSelected, setImportSelected] = React.useState<Set<string>>(new Set());
    const [importLoading, setImportLoading] = React.useState(false);
    const [importSaving, setImportSaving] = React.useState(false);

    // Device selector state — if machineId is passed via route param, use it directly
    const [selectedMachineId, setSelectedMachineId] = React.useState<string | null>(() => {
        if (routeMachineId) return routeMachineId;
        const online = machines.find((m) => isMachineOnline(m));
        console.log(`[ProfileManager] Initial machine selection: ${online?.id || machines[0]?.id || 'none'}, total machines: ${machines.length}`);
        if (online) return online.id;
        return machines.length > 0 ? machines[0].id : null;
    });

    React.useEffect(() => {
        if (routeMachineId) return; // Skip auto-select when machine is specified via route
        if (!selectedMachineId && machines.length > 0) {
            const online = machines.find((m) => isMachineOnline(m));
            console.log(`[ProfileManager] Auto-selecting machine: ${online?.id || machines[0].id}`);
            setSelectedMachineId(online ? online.id : machines[0].id);
        }
    }, [machines, selectedMachineId, routeMachineId]);

    React.useEffect(() => {
        console.log(`[ProfileManager] selectedMachineId changed to: ${selectedMachineId}`);
    }, [selectedMachineId]);

    const loadProfiles = React.useCallback(async () => {
        if (!selectedMachineId) {
            setLoading(false);
            return;
        }
        try {
            console.log(`[ProfileManager] Loading profiles for machine: ${selectedMachineId}`);
            setLoading(true);
            const result = await machineListProfiles(selectedMachineId);
            console.log(`[ProfileManager] Received ${result.profiles?.length || 0} profiles for machine ${selectedMachineId}`, result);
            setProfiles(result.profiles || []);
            setDefaultProfileId(result.defaultProfileId || 'default');
        } catch (e) {
            console.warn('[ProfileManager] Failed to load profiles:', e);
        } finally {
            setLoading(false);
        }
    }, [selectedMachineId]);

    React.useEffect(() => {
        loadProfiles();
    }, [loadProfiles]);

    const handleAddProfile = () => {
        setEditingProfile({
            id: randomUUID(),
            name: '',
            chat: {
                api_key: '',
                base_url: 'https://api.deepseek.com/anthropic',
                model: 'deepseek-reasoner',
                temperature: 0.7,
                max_tokens: 8192,
                context_window_tokens: undefined,
            },
            compress: {
                api_key: '',
                base_url: 'https://api.deepseek.com/anthropic',
                model: 'deepseek-chat',
                temperature: 0.3,
                max_tokens: 4096,
                context_window_tokens: undefined,
            },
        });
        setIsEditingExisting(false);
        setShowEditForm(true);
    };

    const handleEditProfile = (display: ModelOptionDisplay) => {
        // Convert display to input (api_key will be empty - user fills if changing)
        setEditingProfile({
            id: display.id,
            name: display.name,
            chat: {
                api_key: '', // Don't prefill masked key
                base_url: display.chat.base_url,
                model: display.chat.model,
                temperature: display.chat.temperature,
                max_tokens: display.chat.max_tokens,
                context_window_tokens: display.chat.context_window_tokens,
            },
            compress: {
                api_key: '',
                base_url: display.compress.base_url,
                model: display.compress.model,
                temperature: display.compress.temperature,
                max_tokens: display.compress.max_tokens,
                context_window_tokens: display.compress.context_window_tokens,
            },
            supports_vision: display.supportsVision,
            supports_computer_use: display.supportsComputerUse,
            thinking: display.thinking,
            supports_function_calling: display.supportsFunctionCalling,
            supports_image_output: display.supportsImageOutput,
            api_format: display.apiFormat,
        });
        setIsEditingExisting(true);
        setShowEditForm(true);
    };

    const handleOpenCodingPlan = () => {
        const provider = getCodingPlanProvider(codingPlanProviderId);
        setCodingPlanRegion(provider.defaultRegion);
        setCodingPlanApiKey('');
        setShowCodingPlanModal(true);
    };

    const handleSaveCodingPlan = async () => {
        if (!selectedMachineId) return;
        const apiKey = codingPlanApiKey.trim();
        if (!apiKey) return;

        const provider = getCodingPlanProvider(codingPlanProviderId);
        const { profiles, defaultProfileId } = buildCodingPlanProfiles(provider, codingPlanRegion, apiKey);
        try {
            setCodingPlanSaving(true);
            const result = await machineSaveCodingPlanProfiles(selectedMachineId, profiles, defaultProfileId);
            if (result.success) {
                setShowCodingPlanModal(false);
                setCodingPlanApiKey('');
                loadProfiles();
                Alert.alert('', `已添加 ${provider.name} 的 ${profiles.length} 个模型`);
            } else {
                Alert.alert(t('common.error'), result.error || t('profiles.failedToSave'));
            }
        } catch (e) {
            Alert.alert(t('common.error'), e instanceof Error ? e.message : t('profiles.failedToSave'));
        } finally {
            setCodingPlanSaving(false);
        }
    };

    const handleSaveProfile = async () => {
        if (!editingProfile || !selectedMachineId) return;
        if (!editingProfile.name.trim()) return;

        try {
            const result = await machineSaveProfile(selectedMachineId, editingProfile);
            if (result.success) {
                setShowEditForm(false);
                setEditingProfile(null);
                loadProfiles();
            } else {
                Alert.alert(t('common.error'), result.error || t('profiles.failedToSave'));
            }
        } catch (e) {
            Alert.alert(t('common.error'), e instanceof Error ? e.message : t('profiles.failedToSave'));
        }
    };

    const handleDeleteProfile = async (profile: ModelOptionDisplay) => {
        if (!selectedMachineId) return;

        const confirmed = await Modal.confirm(
            t('profiles.delete.title'),
            t('profiles.delete.message', { name: profile.name }),
            {
                cancelText: t('profiles.delete.cancel'),
                confirmText: t('profiles.delete.confirm'),
                destructive: true,
            }
        );
        if (!confirmed) return;

        try {
            const result = await machineDeleteProfile(selectedMachineId, profile.id);
            if (result.success) {
                loadProfiles();
            } else {
                Modal.alert(t('common.error'), result.error || t('profiles.cannotDelete'));
            }
        } catch (e) {
            Modal.alert(t('common.error'), e instanceof Error ? e.message : t('profiles.failedToDelete'));
        }
    };

    const handleOpenImport = () => {
        // Pick a source device: first online device that isn't the current target
        const otherOnline = machines.filter(m => m.id !== selectedMachineId && isMachineOnline(m));
        setImportSourceId(otherOnline.length > 0 ? otherOnline[0].id : null);
        setImportProfiles([]);
        setImportSelected(new Set());
        setShowImportModal(true);
    };

    const handleLoadImportProfiles = React.useCallback(async () => {
        if (!importSourceId) return;
        try {
            setImportLoading(true);
            const result = await machineExportProfiles(importSourceId);
            setImportProfiles(result.profiles || []);
            setImportSelected(new Set((result.profiles || []).map(p => p.id)));
        } catch (e) {
            console.warn('[ProfileManager] Failed to export profiles:', e);
            Alert.alert(t('common.error'), e instanceof Error ? e.message : 'Failed to load profiles');
        } finally {
            setImportLoading(false);
        }
    }, [importSourceId]);

    React.useEffect(() => {
        if (showImportModal && importSourceId) {
            handleLoadImportProfiles();
        }
    }, [showImportModal, importSourceId, handleLoadImportProfiles]);

    const handleImportSelected = async () => {
        if (!selectedMachineId || importSelected.size === 0) return;
        setImportSaving(true);
        let imported = 0;
        const existingIds = new Set(profiles.map(p => p.id));

        for (const profile of importProfiles) {
            if (!importSelected.has(profile.id)) continue;

            let profileToSave: LlmProfileInput = { ...profile };

            // If a profile with the same ID already exists, create a copy with a new ID
            if (existingIds.has(profile.id)) {
                profileToSave = {
                    ...profile,
                    id: randomUUID(),
                    name: `${profile.name} (${t('profiles.import.createCopy').toLowerCase()})`,
                };
            }

            try {
                const result = await machineSaveProfile(selectedMachineId, profileToSave);
                if (result.success) imported++;
            } catch (e) {
                console.warn(`[ProfileManager] Failed to import profile ${profile.name}:`, e);
            }
        }

        setImportSaving(false);
        setShowImportModal(false);
        if (imported > 0) {
            loadProfiles();
            Alert.alert('', t('profiles.import.success', { count: imported }));
        }
    };

    if (machines.length === 0) {
        return (
            <View style={{ flex: 1, justifyContent: 'center', alignItems: 'center', backgroundColor: theme.colors.surface }}>
                <Text style={{ color: theme.colors.textSecondary, ...Typography.default() }}>
                    {t('profiles.noMachineConnected')}
                </Text>
            </View>
        );
    }

    return (
        <View style={{ flex: 1, backgroundColor: theme.colors.surface }}>
            <ScrollView
                style={{ flex: 1 }}
                contentContainerStyle={{
                    paddingHorizontal: screenWidth > 700 ? 16 : 8,
                    paddingBottom: safeArea.bottom + 100,
                }}
            >
                <View style={[{ maxWidth: layout.maxWidth, alignSelf: 'center', width: '100%' }]}>
                    <Text style={{
                        fontSize: 24,
                        fontWeight: 'bold',
                        color: theme.colors.text,
                        marginVertical: 16,
                        ...Typography.default('semiBold')
                    }}>
                        {t('profiles.title')}
                    </Text>

                    <Text style={{
                        fontSize: 13,
                        color: theme.colors.textSecondary,
                        marginBottom: 16,
                        ...Typography.default()
                    }}>
                        {t('profiles.storageHint')}
                    </Text>

                    {!routeMachineId && machines.length > 1 && (
                        <View style={{ marginBottom: 16 }}>
                            <Text
                                style={{
                                    fontSize: 13,
                                    color: theme.colors.textSecondary,
                                    marginBottom: 8,
                                    ...Typography.default('semiBold'),
                                }}
                            >
                                Device
                            </Text>
                            <ScrollView horizontal showsHorizontalScrollIndicator={false}>
                                {machines.map((m) => {
                                    const selected = m.id === selectedMachineId;
                                    const online = isMachineOnline(m);
                                    return (
                                        <Pressable
                                            key={m.id}
                                            onPress={() => {
                                                console.log(`[ProfileManager] Machine selector clicked: ${m.id} (${m.metadata?.host || 'unknown'})`);
                                                setSelectedMachineId(m.id);
                                            }}
                                            style={{
                                                flexDirection: 'row',
                                                alignItems: 'center',
                                                marginRight: 8,
                                                borderRadius: 8,
                                                borderWidth: 1,
                                                borderColor: selected
                                                    ? theme.colors.textLink
                                                    : theme.colors.divider,
                                                backgroundColor: selected
                                                    ? theme.colors.textLink
                                                    : 'transparent',
                                                paddingVertical: 8,
                                                paddingHorizontal: 12,
                                            }}
                                        >
                                            <View
                                                style={{
                                                    width: 6,
                                                    height: 6,
                                                    borderRadius: 3,
                                                    backgroundColor: online ? '#34C759' : '#8E8E93',
                                                    marginRight: 6,
                                                }}
                                            />
                                            <Text
                                                style={{
                                                    fontSize: 13,
                                                    color: selected ? '#fff' : theme.colors.text,
                                                    ...Typography.default('semiBold'),
                                                }}
                                            >
                                                {m.decryptionFailed
                                                    ? '🔐 需要密钥'
                                                    : (m.metadata?.displayName ||
                                                       m.metadata?.host ||
                                                       m.id.slice(0, 8))}
                                            </Text>
                                        </Pressable>
                                    );
                                })}
                            </ScrollView>
                        </View>
                    )}

                    {loading ? (
                        <ActivityIndicator style={{ marginTop: 40 }} />
                    ) : (
                        <>
                            {/* Proxy profiles (read-only) */}
                            {profiles.some(p => p.isProxy) && (
                                <>
                                    <Text style={{
                                        fontSize: 13, color: theme.colors.textSecondary,
                                        marginBottom: 8, ...Typography.default('semiBold'),
                                    }}>内置代理模型</Text>
                                    {profiles.filter(p => p.isProxy).map((profile) => (
                                        <View
                                            key={profile.id}
                                            style={{
                                                backgroundColor: theme.colors.input.background,
                                                borderRadius: 12,
                                                padding: 16,
                                                marginBottom: 8,
                                                flexDirection: 'row',
                                                alignItems: 'center',
                                                opacity: 0.7,
                                            }}
                                        >
                                            {(() => {
                                                const avatarKey = getDefaultAvatarForModelId(profile.id);
                                                const avatarUri = avatarKey ? MODEL_AVATAR_IMAGES[avatarKey] : null;
                                                return avatarUri ? (
                                                    <Image
                                                        source={{ uri: avatarUri }}
                                                        style={{ width: 28, height: 28, borderRadius: 14, marginRight: 12 }}
                                                        contentFit="cover"
                                                    />
                                                ) : (
                                                    <View style={{
                                                        width: 28, height: 28, borderRadius: 14,
                                                        backgroundColor: theme.colors.success,
                                                        justifyContent: 'center', alignItems: 'center', marginRight: 12,
                                                    }}>
                                                        <Ionicons name="flash-outline" size={14} color="white" />
                                                    </View>
                                                );
                                            })()}
                                            <View style={{ flex: 1 }}>
                                                <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                                                    <Text style={{
                                                        fontSize: 16, fontWeight: '600',
                                                        color: theme.colors.text, ...Typography.default('semiBold'),
                                                    }}>{profile.name}</Text>
                                                    {profile.supportsVision && (
                                                        <Ionicons name="image-outline" size={14} color={theme.colors.textSecondary} />
                                                    )}
                                                    {profile.supportsComputerUse && (
                                                        <Ionicons name="desktop-outline" size={14} color={theme.colors.textSecondary} />
                                                    )}
                                                </View>
                                                <Text style={{
                                                    fontSize: 13, color: theme.colors.textSecondary, marginTop: 2,
                                                    ...Typography.default(),
                                                }}>{profile.chat.model}</Text>
                                            </View>
                                            <Ionicons name="lock-closed-outline" size={16} color={theme.colors.textSecondary} />
                                        </View>
                                    ))}
                                </>
                            )}

                            {/* User BYOK profiles (editable) */}
                            {profiles.some(p => !p.isProxy) && (
                                <Text style={{
                                    fontSize: 13, color: theme.colors.textSecondary,
                                    marginTop: profiles.some(p => p.isProxy) ? 16 : 0,
                                    marginBottom: 8, ...Typography.default('semiBold'),
                                }}>自定义 Profile</Text>
                            )}
                            {profiles.filter(p => !p.isProxy).map((profile) => (
                                <View
                                    key={profile.id}
                                    style={{
                                        backgroundColor: theme.colors.input.background,
                                        borderRadius: 12,
                                        padding: 16,
                                        marginBottom: 12,
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        borderWidth: profile.id === defaultProfileId ? 2 : 0,
                                        borderColor: theme.colors.text,
                                    }}
                                >
                                    <Pressable
                                        style={{
                                            flex: 1,
                                            flexDirection: 'row',
                                            alignItems: 'center',
                                        }}
                                        onPress={() => handleEditProfile(profile)}
                                    >
                                        <View style={{
                                            width: 24,
                                            height: 24,
                                            borderRadius: 12,
                                            backgroundColor: profile.id === defaultProfileId
                                                ? theme.colors.button.primary.background
                                                : theme.colors.button.secondary.tint,
                                            justifyContent: 'center',
                                            alignItems: 'center',
                                            marginRight: 12,
                                        }}>
                                            <Ionicons
                                                name={profile.id === defaultProfileId ? 'star' : 'person'}
                                                size={16}
                                                color="white"
                                            />
                                        </View>
                                        <View style={{ flex: 1 }}>
                                            <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                                                <Text style={{
                                                    fontSize: 16,
                                                    fontWeight: '600',
                                                    color: theme.colors.text,
                                                    ...Typography.default('semiBold')
                                                }}>
                                                    {profile.name}
                                                </Text>
                                                {profile.supportsVision && (
                                                    <Ionicons name="image-outline" size={14} color={theme.colors.textSecondary} />
                                                )}
                                                {profile.supportsComputerUse && (
                                                    <Ionicons name="desktop-outline" size={14} color={theme.colors.textSecondary} />
                                                )}
                                            </View>
                                            <Text style={{
                                                fontSize: 13,
                                                color: theme.colors.textSecondary,
                                                marginTop: 2,
                                                ...Typography.default()
                                            }}>
                                                {t('profiles.chatCompressSummary', {
                                                    chatModel: profile.chat.model,
                                                    compressModel: profile.compress.model,
                                                })}
                                            </Text>
                                            <Text style={{
                                                fontSize: 12,
                                                color: theme.colors.textSecondary,
                                                marginTop: 1,
                                                ...Typography.default()
                                            }}>
                                                {t('profiles.authToken')}: {profile.chat.api_key_masked}
                                            </Text>
                                        </View>
                                    </Pressable>
                                    <View style={{ flexDirection: 'row', alignItems: 'center' }}>
                                        {profile.id !== defaultProfileId && (
                                            <Pressable
                                                hitSlop={{ top: 10, bottom: 10, left: 10, right: 10 }}
                                                onPress={() => {
                                                    void handleDeleteProfile(profile);
                                                }}
                                                style={{ marginLeft: 12 }}
                                            >
                                                <Ionicons name="trash-outline" size={20} color={theme.colors.deleteAction} />
                                            </Pressable>
                                        )}
                                    </View>
                                </View>
                            ))}

                            {/* Add profile button */}
                            <Pressable
                                style={{
                                    backgroundColor: theme.colors.surface,
                                    borderRadius: 12,
                                    padding: 16,
                                    marginBottom: 12,
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                }}
                                onPress={handleAddProfile}
                            >
                                <Ionicons name="add-circle-outline" size={20} color={theme.colors.button.secondary.tint} />
                                <Text style={{
                                    fontSize: 16,
                                    fontWeight: '600',
                                    color: theme.colors.button.secondary.tint,
                                    marginLeft: 8,
                                    ...Typography.default('semiBold')
                                }}>
                                    {t('profiles.addProfile')}
                                </Text>
                            </Pressable>

                            <Pressable
                                style={{
                                    backgroundColor: theme.colors.surface,
                                    borderRadius: 12,
                                    padding: 16,
                                    marginBottom: 12,
                                    flexDirection: 'row',
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                }}
                                onPress={handleOpenCodingPlan}
                            >
                                <Ionicons name="code-slash-outline" size={20} color={theme.colors.button.secondary.tint} />
                                <Text style={{
                                    fontSize: 16,
                                    fontWeight: '600',
                                    color: theme.colors.button.secondary.tint,
                                    marginLeft: 8,
                                    ...Typography.default('semiBold')
                                }}>
                                    添加 Coding Plan
                                </Text>
                            </Pressable>

                            {/* Import from another device (only show when multiple machines exist) */}
                            {machines.length > 1 && (
                                <Pressable
                                    style={{
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 12,
                                        padding: 16,
                                        marginBottom: 12,
                                        flexDirection: 'row',
                                        alignItems: 'center',
                                        justifyContent: 'center',
                                    }}
                                    onPress={handleOpenImport}
                                >
                                    <Ionicons name="download-outline" size={20} color={theme.colors.button.secondary.tint} />
                                    <Text style={{
                                        fontSize: 16,
                                        fontWeight: '600',
                                        color: theme.colors.button.secondary.tint,
                                        marginLeft: 8,
                                        ...Typography.default('semiBold')
                                    }}>
                                        {t('profiles.import.button')}
                                    </Text>
                                </Pressable>
                            )}
                        </>
                    )}
                </View>
            </ScrollView>

            {/* Coding Plan Preset Modal */}
            {showCodingPlanModal && (() => {
                const provider = getCodingPlanProvider(codingPlanProviderId);
                const preview = buildCodingPlanProfiles(provider, codingPlanRegion, codingPlanApiKey.trim() || 'sk-...');
                return (
                    <View style={profileManagerStyles.modalOverlay}>
                        <View style={profileManagerStyles.modalContent}>
                            <ScrollView style={{ flex: 1 }} contentContainerStyle={{ padding: 20 }}>
                                <Text style={{
                                    fontSize: 18,
                                    fontWeight: 'bold',
                                    color: theme.colors.text,
                                    marginBottom: 16,
                                    ...Typography.default('semiBold')
                                }}>
                                    添加 Coding Plan
                                </Text>

                                <Text style={{
                                    fontSize: 13,
                                    color: theme.colors.textSecondary,
                                    marginBottom: 8,
                                    ...Typography.default('semiBold')
                                }}>
                                    选择套餐
                                </Text>
                                <View style={{ gap: 8, marginBottom: 16 }}>
                                    {CODING_PLAN_PRESETS.map((item) => {
                                        const selected = item.id === codingPlanProviderId;
                                        return (
                                            <Pressable
                                                key={item.id}
                                                onPress={() => {
                                                    setCodingPlanProviderId(item.id);
                                                    setCodingPlanRegion(item.defaultRegion);
                                                }}
                                                style={{
                                                    borderRadius: 10,
                                                    padding: 12,
                                                    backgroundColor: selected ? theme.colors.button.primary.background : theme.colors.input.background,
                                                    borderWidth: 1,
                                                    borderColor: selected ? theme.colors.button.primary.background : theme.colors.divider,
                                                }}
                                            >
                                                <Text style={{
                                                    fontSize: 15,
                                                    color: selected ? theme.colors.button.primary.tint : theme.colors.text,
                                                    ...Typography.default('semiBold'),
                                                }}>
                                                    {item.name}
                                                </Text>
                                                <Text style={{
                                                    fontSize: 12,
                                                    color: selected ? theme.colors.button.primary.tint : theme.colors.textSecondary,
                                                    marginTop: 4,
                                                    ...Typography.default(),
                                                }}>
                                                    {item.description}
                                                </Text>
                                            </Pressable>
                                        );
                                    })}
                                </View>

                                {provider.regions.length > 1 && (
                                    <>
                                        <Text style={{
                                            fontSize: 13,
                                            color: theme.colors.textSecondary,
                                            marginBottom: 8,
                                            ...Typography.default('semiBold')
                                        }}>
                                            API 区域
                                        </Text>
                                        <View style={{ flexDirection: 'row', gap: 8, marginBottom: 16 }}>
                                            {provider.regions.map((region) => {
                                                const selected = region.id === codingPlanRegion;
                                                return (
                                                    <Pressable
                                                        key={region.id}
                                                        onPress={() => setCodingPlanRegion(region.id)}
                                                        style={{
                                                            flex: 1,
                                                            backgroundColor: selected ? theme.colors.button.primary.background : theme.colors.surfaceHigh,
                                                            borderRadius: 8,
                                                            padding: 10,
                                                            alignItems: 'center',
                                                        }}
                                                    >
                                                        <Text style={{
                                                            fontSize: 14,
                                                            color: selected ? theme.colors.button.primary.tint : theme.colors.text,
                                                            ...Typography.default('semiBold'),
                                                        }}>
                                                            {region.label}
                                                        </Text>
                                                    </Pressable>
                                                );
                                            })}
                                        </View>
                                    </>
                                )}

                                <ProfileField
                                    label="Coding Plan SK"
                                    value={codingPlanApiKey}
                                    onChangeText={setCodingPlanApiKey}
                                    placeholder="sk-..."
                                    secureTextEntry
                                />

                                <View style={{
                                    backgroundColor: theme.colors.input.background,
                                    borderRadius: 10,
                                    padding: 12,
                                    marginTop: 4,
                                }}>
                                    <Text style={{
                                        fontSize: 13,
                                        color: theme.colors.text,
                                        marginBottom: 8,
                                        ...Typography.default('semiBold'),
                                    }}>
                                        将创建 {preview.profiles.length} 个模型，默认 {preview.defaultProfileId.replace(/^coding-plan-[^-]+-/, '')}
                                    </Text>
                                    {preview.profiles.slice(0, 8).map((profile) => (
                                        <Text
                                            key={profile.id}
                                            style={{
                                                fontSize: 12,
                                                color: theme.colors.textSecondary,
                                                marginBottom: 3,
                                                ...Typography.default(),
                                            }}
                                        >
                                            {profile.chat.model} · {profile.chat.context_window_tokens ? `${profile.chat.context_window_tokens / 1000}K` : 'auto'}
                                        </Text>
                                    ))}
                                </View>

                                <View style={{ flexDirection: 'row', gap: 12, marginTop: 24 }}>
                                    <Pressable
                                        style={{
                                            flex: 1,
                                            backgroundColor: theme.colors.surface,
                                            borderRadius: 8,
                                            padding: 12,
                                            alignItems: 'center',
                                        }}
                                        onPress={() => { setShowCodingPlanModal(false); setCodingPlanApiKey(''); }}
                                    >
                                        <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.secondary.tint, ...Typography.default('semiBold') }}>
                                            {t('common.cancel')}
                                        </Text>
                                    </Pressable>
                                    <Pressable
                                        style={{
                                            flex: 1,
                                            backgroundColor: codingPlanApiKey.trim() ? theme.colors.button.primary.background : theme.colors.divider,
                                            borderRadius: 8,
                                            padding: 12,
                                            alignItems: 'center',
                                            opacity: codingPlanApiKey.trim() ? 1 : 0.5,
                                        }}
                                        onPress={handleSaveCodingPlan}
                                        disabled={!codingPlanApiKey.trim() || codingPlanSaving}
                                    >
                                        {codingPlanSaving ? (
                                            <ActivityIndicator color={theme.colors.button.primary.tint} />
                                        ) : (
                                            <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.primary.tint, ...Typography.default('semiBold') }}>
                                                添加并设为默认
                                            </Text>
                                        )}
                                    </Pressable>
                                </View>
                            </ScrollView>
                        </View>
                    </View>
                );
            })()}

            {/* Profile Edit Modal */}
            {showEditForm && editingProfile && (
                <View style={profileManagerStyles.modalOverlay}>
                    <View style={profileManagerStyles.modalContent}>
                        <ScrollView style={{ flex: 1 }} contentContainerStyle={{ padding: 20 }}>
                            <Text style={{
                                fontSize: 18,
                                fontWeight: 'bold',
                                color: theme.colors.text,
                                marginBottom: 20,
                                ...Typography.default('semiBold')
                            }}>
                                {editingProfile.name ? t('profiles.editProfile') : t('profiles.addProfileTitle')}
                            </Text>

                            <ProfileField
                                label={t('profiles.profileName')}
                                value={editingProfile.name}
                                onChangeText={(v) => setEditingProfile({ ...editingProfile, name: v })}
                                placeholder={t('profiles.enterName')}
                            />

                            <Text style={{
                                fontSize: 15,
                                fontWeight: '600',
                                color: theme.colors.text,
                                marginTop: 20,
                                marginBottom: 8,
                                ...Typography.default('semiBold')
                            }}>{t('profiles.chatModel')}</Text>

                            <EndpointEditor
                                endpoint={editingProfile.chat}
                                onChange={(chat) => setEditingProfile({ ...editingProfile, chat })}
                                apiKeyPlaceholder={isEditingExisting ? t('profiles.keepExistingKey') : "sk-..."}
                                showContextWindowTokens
                            />

                            <Text style={{
                                fontSize: 15,
                                fontWeight: '600',
                                color: theme.colors.text,
                                marginTop: 20,
                                marginBottom: 8,
                                ...Typography.default('semiBold')
                            }}>{t('profiles.compressModel')}</Text>

                            <EndpointEditor
                                endpoint={editingProfile.compress}
                                onChange={(compress) => setEditingProfile({ ...editingProfile, compress })}
                                apiKeyPlaceholder={isEditingExisting ? t('profiles.keepExistingKey') : "sk-..."}
                            />

                            {/* Capabilities */}
                            <Text style={{
                                fontSize: 15,
                                fontWeight: '600',
                                color: theme.colors.text,
                                marginTop: 20,
                                marginBottom: 12,
                                ...Typography.default('semiBold')
                            }}>模型能力</Text>

                            {/* Supports Vision */}
                            <Pressable
                                style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 12 }}
                                onPress={() => setEditingProfile({ ...editingProfile, supports_vision: !editingProfile.supports_vision })}
                            >
                                <View style={{
                                    width: 20, height: 20, borderRadius: 4,
                                    borderWidth: 2,
                                    borderColor: editingProfile.supports_vision ? theme.colors.button.primary.background : theme.colors.textSecondary,
                                    backgroundColor: editingProfile.supports_vision ? theme.colors.button.primary.background : 'transparent',
                                    justifyContent: 'center', alignItems: 'center', marginRight: 8,
                                }}>
                                    {editingProfile.supports_vision && (
                                        <Ionicons name="checkmark" size={12} color={theme.colors.button.primary.tint} />
                                    )}
                                </View>
                                <Ionicons name="image-outline" size={16} color={theme.colors.text} style={{ marginRight: 6 }} />
                                <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default() }}>
                                    支持图像输入（Vision）
                                </Text>
                            </Pressable>

                            {/* Supports Computer Use */}
                            <Pressable
                                style={{ flexDirection: 'row', alignItems: 'center', marginBottom: 12 }}
                                onPress={() => setEditingProfile({ ...editingProfile, supports_computer_use: !editingProfile.supports_computer_use })}
                            >
                                <View style={{
                                    width: 20, height: 20, borderRadius: 4,
                                    borderWidth: 2,
                                    borderColor: editingProfile.supports_computer_use ? theme.colors.button.primary.background : theme.colors.textSecondary,
                                    backgroundColor: editingProfile.supports_computer_use ? theme.colors.button.primary.background : 'transparent',
                                    justifyContent: 'center', alignItems: 'center', marginRight: 8,
                                }}>
                                    {editingProfile.supports_computer_use && (
                                        <Ionicons name="checkmark" size={12} color={theme.colors.button.primary.tint} />
                                    )}
                                </View>
                                <Ionicons name="desktop-outline" size={16} color={theme.colors.text} style={{ marginRight: 6 }} />
                                <Text style={{ fontSize: 14, color: theme.colors.text, ...Typography.default() }}>
                                    支持桌面操控（Computer Use）
                                </Text>
                            </Pressable>

                            {/* API Format */}
                            <Text style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default()
                            }}>API 兼容格式</Text>
                            <View style={{ flexDirection: 'row', gap: 8, marginBottom: 16 }}>
                                {([['anthropic', 'Anthropic'], ['openai', 'OpenAI'], ['gemini', 'Gemini']] as const).map(([value, label]) => (
                                    <Pressable
                                        key={value}
                                        onPress={() => setEditingProfile({ ...editingProfile, api_format: value })}
                                        style={{
                                            flex: 1,
                                            backgroundColor: (editingProfile.api_format || 'anthropic') === value
                                                ? theme.colors.button.primary.background
                                                : theme.colors.surfaceHigh,
                                            borderRadius: 8,
                                            padding: 10,
                                            alignItems: 'center',
                                        }}
                                    >
                                        <Text style={{
                                            fontSize: 14,
                                            color: (editingProfile.api_format || 'anthropic') === value
                                                ? theme.colors.button.primary.tint
                                                : theme.colors.text,
                                            ...Typography.default('semiBold'),
                                        }}>{label}</Text>
                                    </Pressable>
                                ))}
                            </View>

                            <View style={{ flexDirection: 'row', gap: 12, marginTop: 24 }}>
                                <Pressable
                                    style={{
                                        flex: 1,
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 12,
                                        alignItems: 'center',
                                    }}
                                    onPress={() => { setShowEditForm(false); setEditingProfile(null); }}
                                >
                                    <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.secondary.tint, ...Typography.default('semiBold') }}>
                                        {t('common.cancel')}
                                    </Text>
                                </Pressable>
                                <Pressable
                                    style={{
                                        flex: 1,
                                        backgroundColor: theme.colors.button.primary.background,
                                        borderRadius: 8,
                                        padding: 12,
                                        alignItems: 'center',
                                    }}
                                    onPress={handleSaveProfile}
                                >
                                    <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.primary.tint, ...Typography.default('semiBold') }}>
                                        {t('common.save')}
                                    </Text>
                                </Pressable>
                            </View>
                        </ScrollView>
                    </View>
                </View>
            )}

            {/* Import Profiles Modal */}
            {showImportModal && (
                <View style={profileManagerStyles.modalOverlay}>
                    <View style={profileManagerStyles.modalContent}>
                        <ScrollView style={{ flex: 1 }} contentContainerStyle={{ padding: 20 }}>
                            <Text style={{
                                fontSize: 18,
                                fontWeight: 'bold',
                                color: theme.colors.text,
                                marginBottom: 16,
                                ...Typography.default('semiBold')
                            }}>
                                {t('profiles.import.title')}
                            </Text>

                            {/* Source device selector */}
                            <Text style={{
                                fontSize: 13,
                                color: theme.colors.textSecondary,
                                marginBottom: 8,
                                ...Typography.default('semiBold')
                            }}>
                                {t('profiles.import.selectSource')}
                            </Text>
                            <ScrollView horizontal showsHorizontalScrollIndicator={false} style={{ marginBottom: 16 }}>
                                {machines
                                    .filter(m => m.id !== selectedMachineId && isMachineOnline(m))
                                    .map(m => {
                                        const selected = m.id === importSourceId;
                                        return (
                                            <Pressable
                                                key={m.id}
                                                onPress={() => setImportSourceId(m.id)}
                                                style={{
                                                    flexDirection: 'row',
                                                    alignItems: 'center',
                                                    marginRight: 8,
                                                    borderRadius: 8,
                                                    borderWidth: 1,
                                                    borderColor: selected ? theme.colors.textLink : theme.colors.divider,
                                                    backgroundColor: selected ? theme.colors.textLink : 'transparent',
                                                    paddingVertical: 8,
                                                    paddingHorizontal: 12,
                                                }}
                                            >
                                                <View style={{
                                                    width: 6, height: 6, borderRadius: 3,
                                                    backgroundColor: '#34C759', marginRight: 6,
                                                }} />
                                                <Text style={{
                                                    fontSize: 13,
                                                    color: selected ? '#fff' : theme.colors.text,
                                                    ...Typography.default('semiBold'),
                                                }}>
                                                    {m.metadata?.displayName || m.metadata?.host || m.id.slice(0, 8)}
                                                </Text>
                                            </Pressable>
                                        );
                                    })}
                            </ScrollView>

                            {importLoading ? (
                                <ActivityIndicator style={{ marginTop: 20 }} />
                            ) : importProfiles.length === 0 ? (
                                <Text style={{ color: theme.colors.textSecondary, marginTop: 20, ...Typography.default() }}>
                                    {t('profiles.import.noProfiles')}
                                </Text>
                            ) : (
                                <>
                                    {/* Select all / deselect all */}
                                    <Pressable
                                        onPress={() => {
                                            if (importSelected.size === importProfiles.length) {
                                                setImportSelected(new Set());
                                            } else {
                                                setImportSelected(new Set(importProfiles.map(p => p.id)));
                                            }
                                        }}
                                        style={{ marginBottom: 12 }}
                                    >
                                        <Text style={{
                                            fontSize: 13,
                                            color: theme.colors.textLink,
                                            ...Typography.default('semiBold')
                                        }}>
                                            {importSelected.size === importProfiles.length
                                                ? t('profiles.import.deselectAll')
                                                : t('profiles.import.selectAll')}
                                        </Text>
                                    </Pressable>

                                    {/* Profile checklist */}
                                    {importProfiles.map(profile => {
                                        const checked = importSelected.has(profile.id);
                                        const existsLocally = profiles.some(p => p.id === profile.id);
                                        return (
                                            <Pressable
                                                key={profile.id}
                                                onPress={() => {
                                                    const next = new Set(importSelected);
                                                    if (checked) next.delete(profile.id);
                                                    else next.add(profile.id);
                                                    setImportSelected(next);
                                                }}
                                                style={{
                                                    flexDirection: 'row',
                                                    alignItems: 'center',
                                                    backgroundColor: theme.colors.input.background,
                                                    borderRadius: 10,
                                                    padding: 12,
                                                    marginBottom: 8,
                                                }}
                                            >
                                                <Ionicons
                                                    name={checked ? 'checkbox' : 'square-outline'}
                                                    size={22}
                                                    color={checked ? theme.colors.textLink : theme.colors.textSecondary}
                                                    style={{ marginRight: 10 }}
                                                />
                                                <View style={{ flex: 1 }}>
                                                    <Text style={{ fontSize: 15, color: theme.colors.text, ...Typography.default('semiBold') }}>
                                                        {profile.name}
                                                    </Text>
                                                    <Text style={{ fontSize: 12, color: theme.colors.textSecondary, marginTop: 2, ...Typography.default() }}>
                                                        {profile.chat.model} / {profile.compress.model}
                                                    </Text>
                                                    {existsLocally && (
                                                        <Text style={{ fontSize: 11, color: theme.colors.warning || '#FF9500', marginTop: 2, ...Typography.default() }}>
                                                            {t('profiles.import.conflict')} — {t('profiles.import.createCopy').toLowerCase()}
                                                        </Text>
                                                    )}
                                                </View>
                                            </Pressable>
                                        );
                                    })}
                                </>
                            )}

                            {/* Action buttons */}
                            <View style={{ flexDirection: 'row', gap: 12, marginTop: 20 }}>
                                <Pressable
                                    style={{
                                        flex: 1,
                                        backgroundColor: theme.colors.surface,
                                        borderRadius: 8,
                                        padding: 12,
                                        alignItems: 'center',
                                    }}
                                    onPress={() => setShowImportModal(false)}
                                >
                                    <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.secondary.tint, ...Typography.default('semiBold') }}>
                                        {t('common.cancel')}
                                    </Text>
                                </Pressable>
                                <Pressable
                                    style={{
                                        flex: 1,
                                        backgroundColor: importSelected.size === 0 ? theme.colors.divider : theme.colors.button.primary.background,
                                        borderRadius: 8,
                                        padding: 12,
                                        alignItems: 'center',
                                        opacity: importSelected.size === 0 ? 0.5 : 1,
                                    }}
                                    onPress={handleImportSelected}
                                    disabled={importSelected.size === 0 || importSaving}
                                >
                                    {importSaving ? (
                                        <ActivityIndicator color={theme.colors.button.primary.tint} />
                                    ) : (
                                        <Text style={{ fontSize: 16, fontWeight: '600', color: theme.colors.button.primary.tint, ...Typography.default('semiBold') }}>
                                            {t('profiles.import.importSelected')} ({importSelected.size})
                                        </Text>
                                    )}
                                </Pressable>
                            </View>
                        </ScrollView>
                    </View>
                </View>
            )}
        </View>
    );
}

function ProfileField({ label, value, onChangeText, placeholder, secureTextEntry }: {
    label: string;
    value: string;
    onChangeText: (v: string) => void;
    placeholder?: string;
    secureTextEntry?: boolean;
}) {
    const { theme } = useUnistyles();
    return (
        <View style={{ marginBottom: 12 }}>
            <Text style={{ fontSize: 13, color: theme.colors.textSecondary, marginBottom: 4, ...Typography.default() }}>
                {label}
            </Text>
            <TextInput
                style={{
                    backgroundColor: theme.colors.input.background,
                    borderRadius: 8,
                    padding: 10,
                    color: theme.colors.text,
                    fontSize: 14,
                    ...Typography.default(),
                }}
                value={value}
                onChangeText={onChangeText}
                placeholder={placeholder}
                placeholderTextColor={theme.colors.textSecondary}
                secureTextEntry={secureTextEntry}
            />
        </View>
    );
}

function EndpointEditor({ endpoint, onChange, apiKeyPlaceholder, showContextWindowTokens }: {
    endpoint: LlmEndpointInput;
    onChange: (endpoint: LlmEndpointInput) => void;
    apiKeyPlaceholder?: string;
    showContextWindowTokens?: boolean;
}) {
    return (
        <View>
            <ProfileField
                label={t('profiles.authTokenGlobalHint')}
                value={endpoint.api_key}
                onChangeText={(v) => onChange({ ...endpoint, api_key: v })}
                placeholder={apiKeyPlaceholder || "sk-..."}
                secureTextEntry
            />
            <ProfileField
                label={t('profiles.baseURL')}
                value={endpoint.base_url}
                onChangeText={(v) => onChange({ ...endpoint, base_url: v })}
                placeholder="https://api.deepseek.com/anthropic"
            />
            <ProfileField
                label={t('profiles.model')}
                value={endpoint.model}
                onChangeText={(v) => onChange({ ...endpoint, model: v })}
                placeholder="deepseek-reasoner"
            />
            <ProfileField
                label={t('profiles.temperature')}
                value={String(endpoint.temperature)}
                onChangeText={(v) => onChange({ ...endpoint, temperature: parseFloat(v) || 0 })}
                placeholder="0.7"
            />
            <ProfileField
                label={t('profiles.maxTokens')}
                value={String(endpoint.max_tokens)}
                onChangeText={(v) => onChange({ ...endpoint, max_tokens: parseInt(v) || 4096 })}
                placeholder="8192"
            />
            {showContextWindowTokens && (
                <ProfileField
                    label="Context Window Tokens (optional)"
                    value={endpoint.context_window_tokens ? String(endpoint.context_window_tokens) : ''}
                    onChangeText={(v) => {
                        const parsed = parseInt(v, 10);
                        onChange({
                            ...endpoint,
                            context_window_tokens: Number.isFinite(parsed) && parsed > 0 ? parsed : undefined,
                        });
                    }}
                    placeholder="e.g. 200000"
                />
            )}
        </View>
    );
}

const profileManagerStyles = StyleSheet.create((theme) => ({
    modalOverlay: {
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        backgroundColor: 'rgba(0, 0, 0, 0.5)',
        justifyContent: 'center',
        alignItems: 'center',
        padding: 20,
    },
    modalContent: {
        width: '100%',
        maxWidth: Math.min(layout.maxWidth, 600),
        maxHeight: '90%',
        backgroundColor: theme.colors.surface,
        borderRadius: 16,
    },
}));

export default ProfileManager;
