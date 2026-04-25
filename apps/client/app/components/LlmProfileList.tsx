import * as React from 'react';
import { View, Pressable } from 'react-native';
import { Image } from 'expo-image';
import { Ionicons } from '@expo/vector-icons';
import { Typography } from '@/constants/Typography';
import { useUnistyles } from 'react-native-unistyles';
import { hapticsLight } from './haptics';
import { Text } from '@/components/StyledText';
import type { ModelOptionDisplay } from '@/sync/ops';
import { MODEL_AVATAR_IMAGES, getDefaultAvatarForModelOption } from '@/utils/modelAvatars';
import { frontendLog } from '@/utils/tauri';

export function isFreeModel(model: ModelOptionDisplay): boolean {
    return model.isFree === true || (model.chat?.model?.includes(':free') ?? false);
}

interface LlmProfileListProps {
    models: ModelOptionDisplay[];
    selectedModelId?: string;
    defaultModelId?: string;
    onModelChange?: (modelId: string) => void;
    variant?: 'inline' | 'modal';
}

export function LlmProfileList({ models, selectedModelId, defaultModelId, onModelChange, variant = 'inline' }: LlmProfileListProps) {
    const { theme } = useUnistyles();

    const vendorProfiles = models.filter(p => p.sourceType === 'vendor');
    const proxyProfiles = models.filter(p => (p.sourceType ?? (p.isProxy ? 'proxy' : 'byok')) === 'proxy');
    const byokProfiles = models.filter(p => (p.sourceType ?? (p.isProxy ? 'proxy' : 'byok')) === 'byok');

    const freeProfiles = proxyProfiles.filter(p => isFreeModel(p));
    const paidProfiles = proxyProfiles.filter(p => !isFreeModel(p));
    const selectedIsFreeProxy = freeProfiles.some((profile) => profile.id === selectedModelId);
    const selectedIsPaidProxy = paidProfiles.some((profile) => profile.id === selectedModelId);
    const [freeCollapsed, setFreeCollapsed] = React.useState(() => freeProfiles.length > 6 && !selectedIsFreeProxy);
    const [paidCollapsed, setPaidCollapsed] = React.useState(() => paidProfiles.length > 6 && !selectedIsPaidProxy);
    const vendorGroups = [
        {
            key: 'claude',
            title: 'Claude Code 模型',
            items: vendorProfiles.filter((profile) => profile.vendor === 'claude'),
        },
        {
            key: 'codex',
            title: 'Codex 模型',
            items: vendorProfiles.filter((profile) => profile.vendor === 'codex'),
        },
        {
            key: 'gemini',
            title: 'Gemini 模型',
            items: vendorProfiles.filter((profile) => profile.vendor === 'gemini'),
        },
        {
            key: 'cteno',
            title: 'Cteno 模型',
            items: vendorProfiles.filter((profile) => profile.vendor === 'cteno'),
        },
    ].filter((group) => group.items.length > 0);

    const hasFree = freeProfiles.length > 0;
    const hasPaid = paidProfiles.length > 0;
    const hasByok = byokProfiles.length > 0;
    const hasVendorGroups = vendorGroups.length > 0;

    React.useEffect(() => {
        if (selectedIsFreeProxy) {
            setFreeCollapsed(false);
        }
        if (selectedIsPaidProxy) {
            setPaidCollapsed(false);
        }
    }, [selectedIsFreeProxy, selectedIsPaidProxy]);

    React.useEffect(() => {
        frontendLog(`[LlmProfileList] ${JSON.stringify({
            variant,
            total: models.length,
            vendorCount: vendorProfiles.length,
            proxyCount: proxyProfiles.length,
            byokCount: byokProfiles.length,
            freeCount: freeProfiles.length,
            paidCount: paidProfiles.length,
            selectedModelId: selectedModelId ?? null,
            defaultModelId: defaultModelId ?? null,
            ids: models.slice(0, 20).map((model) => ({
                id: model.id,
                model: model.chat?.model,
                isProxy: model.isProxy === true,
                sourceType: model.sourceType ?? null,
                vendor: model.vendor ?? null,
            })),
        })}`);
    }, [
        defaultModelId,
        freeProfiles.length,
        models,
        paidProfiles.length,
        vendorProfiles.length,
        proxyProfiles.length,
        byokProfiles.length,
        selectedModelId,
        variant,
    ]);

    if (models.length === 0) return null;

    const renderRow = (model: ModelOptionDisplay) => (
        <ProfileRow
            key={model.id}
            model={model}
            isSelected={selectedModelId === model.id}
            isDefault={defaultModelId === model.id}
            isFree={isFreeModel(model)}
            variant={variant}
            onPress={() => {
                hapticsLight();
                onModelChange?.(model.id);
            }}
        />
    );

    return (
        <View style={{ paddingVertical: 8 }}>
            {vendorGroups.map((group, index) => (
                <React.Fragment key={group.key}>
                    <Text style={{
                        fontSize: 12,
                        fontWeight: '600',
                        color: theme.colors.textSecondary,
                        paddingHorizontal: 16,
                        paddingBottom: variant === 'modal' ? 6 : 4,
                        paddingTop: index > 0 ? (variant === 'modal' ? 12 : 8) : 0,
                        ...Typography.default('semiBold')
                    }}>
                        {group.title}
                    </Text>
                    {group.items.map(renderRow)}
                </React.Fragment>
            ))}

            {hasFree && (
                <>
                    <ProfileSectionHeader
                        title="免费模型"
                        count={freeProfiles.length}
                        collapsed={freeCollapsed}
                        collapsible
                        topPadding={hasVendorGroups ? (variant === 'modal' ? 12 : 8) : 0}
                        variant={variant}
                        onPress={() => setFreeCollapsed((value) => !value)}
                    />
                    {!freeCollapsed && freeProfiles.map(renderRow)}
                </>
            )}

            {hasPaid && (
                <>
                    <ProfileSectionHeader
                        title={variant === 'modal' ? '内置代理模型（消耗余额）' : '内置代理（消耗余额）'}
                        count={paidProfiles.length}
                        collapsed={paidCollapsed}
                        collapsible
                        topPadding={(hasVendorGroups || hasFree) ? (variant === 'modal' ? 12 : 8) : 0}
                        variant={variant}
                        onPress={() => setPaidCollapsed((value) => !value)}
                    />
                    {!paidCollapsed && paidProfiles.map(renderRow)}
                </>
            )}

            {hasByok && (
                <>
                    <Text style={{
                        fontSize: 12,
                        fontWeight: '600',
                        color: theme.colors.textSecondary,
                        paddingHorizontal: 16,
                        paddingBottom: variant === 'modal' ? 6 : 4,
                        paddingTop: (hasVendorGroups || hasFree || hasPaid) ? (variant === 'modal' ? 12 : 8) : 0,
                        ...Typography.default('semiBold')
                    }}>
                        自定义模型（BYOK）
                    </Text>
                    {byokProfiles.map(renderRow)}
                </>
            )}
        </View>
    );
}

function ProfileSectionHeader({ title, count, collapsed, collapsible, topPadding, variant, onPress }: {
    title: string;
    count?: number;
    collapsed?: boolean;
    collapsible?: boolean;
    topPadding: number;
    variant: 'inline' | 'modal';
    onPress?: () => void;
}) {
    const { theme } = useUnistyles();
    const content = (
        <>
            <Text style={{
                fontSize: 12,
                fontWeight: '600',
                color: theme.colors.textSecondary,
                ...Typography.default('semiBold')
            }}>
                {count !== undefined ? `${title} · ${count}` : title}
            </Text>
            {collapsible && (
                <Ionicons
                    name={collapsed ? 'chevron-forward' : 'chevron-down'}
                    size={14}
                    color={theme.colors.textSecondary}
                />
            )}
        </>
    );

    const style = {
        flexDirection: 'row' as const,
        alignItems: 'center' as const,
        gap: 4,
        paddingHorizontal: 16,
        paddingBottom: variant === 'modal' ? 6 : 4,
        paddingTop: topPadding,
    };

    if (!collapsible) {
        return <View style={style}>{content}</View>;
    }

    return (
        <Pressable
            onPress={onPress}
            style={({ pressed }) => [
                style,
                pressed ? { opacity: 0.65 } : null,
            ]}
        >
            {content}
        </Pressable>
    );
}

function ProfileRow({ model, isSelected, isDefault, isFree, variant, onPress }: {
    model: ModelOptionDisplay;
    isSelected: boolean;
    isDefault: boolean;
    isFree: boolean;
    variant: 'inline' | 'modal';
    onPress: () => void;
}) {
    const { theme } = useUnistyles();

    if (variant === 'modal') {
        const avatarKey = getDefaultAvatarForModelOption({
            modelId: model.id,
            vendor: model.vendor,
        });
        const avatarUri = avatarKey ? MODEL_AVATAR_IMAGES[avatarKey] : null;

        return (
            <Pressable
                onPress={onPress}
                style={({ pressed }) => ({
                    flexDirection: 'row',
                    alignItems: 'center',
                    paddingHorizontal: 16,
                    paddingVertical: 10,
                    backgroundColor: pressed
                        ? theme.colors.surfacePressed
                        : isSelected
                            ? theme.colors.groupped.background
                            : 'transparent',
                })}
            >
                {avatarUri ? (
                    <Image
                        source={{ uri: avatarUri }}
                        style={{ width: 24, height: 24, borderRadius: 12, marginRight: 10 }}
                        contentFit="cover"
                    />
                ) : (
                    <View style={{
                        width: 24, height: 24, borderRadius: 12, marginRight: 10,
                        backgroundColor: theme.colors.surfaceHigh,
                        justifyContent: 'center', alignItems: 'center',
                    }}>
                        <Ionicons name="server-outline" size={12} color={theme.colors.textSecondary} />
                    </View>
                )}
                <View style={{ flex: 1 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                        <Text style={{
                            fontSize: 15,
                            color: theme.colors.text,
                            fontWeight: isSelected ? '600' : '400',
                        }}>
                            {model.name}
                        </Text>
                        {model.supportsVision && (
                            <Ionicons name="image-outline" size={14} color={theme.colors.textSecondary} />
                        )}
                        {model.supportsComputerUse && (
                            <Ionicons name="desktop-outline" size={14} color={theme.colors.textSecondary} />
                        )}
                        {isFree && (
                            <View style={{
                                backgroundColor: '#22c55e',
                                borderRadius: 4,
                                paddingHorizontal: 4,
                                paddingVertical: 1,
                            }}>
                                <Text style={{
                                    fontSize: 10,
                                    color: '#fff',
                                    fontWeight: '700',
                                }}>
                                    免费
                                </Text>
                            </View>
                        )}
                    </View>
                    <Text style={{
                        fontSize: 12,
                        color: theme.colors.textSecondary,
                        marginTop: 1,
                    }}>
                        {model.description || model.chat.model}
                    </Text>
                </View>
                {isSelected && (
                    <Ionicons name="checkmark-circle" size={20} color={theme.colors.text} />
                )}
            </Pressable>
        );
    }

    // inline variant (radio button style)
    return (
        <Pressable
            onPress={onPress}
            style={({ pressed }) => ({
                flexDirection: 'row',
                alignItems: 'center',
                paddingHorizontal: 16,
                paddingVertical: 8,
                backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent'
            })}
        >
            <View style={{
                width: 16,
                height: 16,
                borderRadius: 8,
                borderWidth: 2,
                borderColor: isSelected ? theme.colors.radio.active : theme.colors.radio.inactive,
                alignItems: 'center',
                justifyContent: 'center',
                marginRight: 12
            }}>
                {isSelected && (
                    <View style={{
                        width: 6,
                        height: 6,
                        borderRadius: 3,
                        backgroundColor: theme.colors.radio.dot
                    }} />
                )}
            </View>
            <View style={{ flex: 1, flexDirection: 'row', alignItems: 'center' }}>
                <View style={{ flex: 1 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
                        <Text style={{
                            fontSize: 14,
                            color: isSelected ? theme.colors.radio.active : theme.colors.text,
                            ...Typography.default()
                        }}>
                            {model.name}{isDefault ? ' ★' : ''}
                        </Text>
                        {isFree && (
                            <View style={{
                                backgroundColor: '#22c55e',
                                borderRadius: 4,
                                paddingHorizontal: 4,
                                paddingVertical: 1,
                            }}>
                                <Text style={{
                                    fontSize: 10,
                                    color: '#fff',
                                    fontWeight: '700',
                                    ...Typography.default('semiBold')
                                }}>
                                    免费
                                </Text>
                            </View>
                        )}
                    </View>
                    <Text style={{
                        fontSize: 11,
                        color: theme.colors.textSecondary,
                        ...Typography.default()
                    }}>
                        {model.chat.model}
                    </Text>
                </View>
            </View>
        </Pressable>
    );
}
