import React from 'react';
import { View, Pressable, Platform } from 'react-native';
import { Swipeable } from 'react-native-gesture-handler';
import { Image } from 'expo-image';
import { Ionicons } from '@expo/vector-icons';
import { useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import { t } from '@/text';
import type { Persona } from '../sync/storageTypes';
import { isModelAvatar, MODEL_AVATAR_IMAGES, getModelBadgeUri } from '@/utils/modelAvatars';
import { StatusDot } from './StatusDot';
import { getVendorAvatarId, getVendorIconSource } from '@/utils/vendorIcons';

const AVATAR_ICONS: Record<string, string> = {
    default: 'person-circle-outline',
    robot: 'hardware-chip-outline',
    brain: 'bulb-outline',
    star: 'star-outline',
    rocket: 'rocket-outline',
    code: 'code-slash-outline',
    book: 'book-outline',
    search: 'search-outline',
};

function formatRelativeTime(timestamp: number): string {
    const now = Date.now();
    const diff = now - timestamp;
    const seconds = Math.floor(diff / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);

    if (seconds < 60) return t('persona.justNow');
    if (minutes < 60) return t('persona.minutesAgo', { count: minutes });
    if (hours < 24) return t('persona.hoursAgo', { count: hours });
    if (days === 1) return t('persona.yesterday');
    return t('persona.daysAgo', { count: days });
}

interface PersonaCardProps {
    persona: Persona;
    onPress: () => void;
    onDelete?: () => void | Promise<void>;
    onAvatarPress?: () => void;
    lastMessage?: { text: string; isUser: boolean; createdAt: number } | null;
    unreadCount?: number;
    isOffline?: boolean;
    isSelected?: boolean;
    hasScheduledTask?: boolean;
    isThinking?: boolean;
    sessionProfileId?: string;
    workspaceLabel?: string | null;
    deleting?: boolean;
}

export const PersonaCard: React.FC<PersonaCardProps> = ({
    persona,
    onPress,
    onDelete,
    onAvatarPress,
    lastMessage,
    unreadCount = 0,
    isOffline = false,
    isSelected = false,
    hasScheduledTask = false,
    isThinking = false,
    sessionProfileId,
    workspaceLabel,
    deleting = false,
}) => {
    const { theme } = useUnistyles();
    const iconName = AVATAR_ICONS[persona.avatarId] || AVATAR_ICONS.default;
    const suppressModelBadge = persona.agent === 'claude' || persona.agent === 'codex' || persona.agent === 'gemini';
    const primaryAvatarId = React.useMemo(() => {
        if (persona.avatarId !== 'default') {
            return persona.avatarId;
        }
        if (persona.agent) {
            return getVendorAvatarId(persona.agent);
        }
        return persona.avatarId;
    }, [persona.agent, persona.avatarId]);
    const swipeableRef = React.useRef<Swipeable | null>(null);
    const swipeEnabled = Platform.OS !== 'web';
    const [hovered, setHovered] = React.useState(false);
    const [confirmingDelete, setConfirmingDelete] = React.useState(false);
    const hideHoverTimeoutRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

    const previewText = lastMessage
        ? (lastMessage.isUser ? `${t('persona.you')}: ${lastMessage.text}` : lastMessage.text)
        : (persona.description || t('persona.noMessages'));

    const renderRightActions = () => (
        <Pressable
            style={{
                width: 80,
                height: '100%',
                alignItems: 'center',
                justifyContent: 'center',
                backgroundColor: theme.colors.status.error,
            }}
            onPress={() => {
                swipeableRef.current?.close();
                onDelete?.();
            }}
        >
            <Ionicons name="trash-outline" size={20} color="#FFFFFF" />
            <Text style={{
                marginTop: 4,
                fontSize: 12,
                color: '#FFFFFF',
                textAlign: 'center',
                ...Typography.default('semiBold'),
            }}>
                {t('persona.deleteAction')}
            </Text>
        </Pressable>
    );

    const showInlineAction = !swipeEnabled && !!onDelete;
    const showDeleteControls = showInlineAction && (hovered || confirmingDelete);
    const rightPadding = showInlineAction
        ? (confirmingDelete ? 152 : showDeleteControls ? 72 : 16)
        : 16;

    const showHover = React.useCallback(() => {
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
            hideHoverTimeoutRef.current = null;
        }
        setHovered(true);
    }, []);

    const scheduleHideHover = React.useCallback(() => {
        if (confirmingDelete) return;
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
        }
        hideHoverTimeoutRef.current = setTimeout(() => {
            setHovered(false);
            hideHoverTimeoutRef.current = null;
        }, 120);
    }, [confirmingDelete]);

    React.useEffect(() => () => {
        if (hideHoverTimeoutRef.current) {
            clearTimeout(hideHoverTimeoutRef.current);
        }
    }, []);

    const handleCardPress = React.useCallback(() => {
        if (confirmingDelete) {
            setConfirmingDelete(false);
            return;
        }
        onPress();
    }, [confirmingDelete, onPress]);

    const handleConfirmDelete = React.useCallback(async (e?: any) => {
        e?.stopPropagation?.();
        if (!onDelete || deleting) return;
        try {
            await onDelete();
        } finally {
            setConfirmingDelete(false);
        }
    }, [deleting, onDelete]);

    const cardContent = (
        <Pressable
            accessible={false}
            onHoverIn={showInlineAction ? showHover : undefined}
            onHoverOut={showInlineAction ? scheduleHideHover : undefined}
            style={{ position: 'relative', width: '100%', alignSelf: 'stretch' }}
        >
            <Pressable
                onPress={handleCardPress}
                onHoverIn={showInlineAction ? showHover : undefined}
                onHoverOut={showInlineAction ? scheduleHideHover : undefined}
                style={({ pressed }) => ({
                    backgroundColor: pressed
                        ? theme.colors.surfacePressed
                        : isSelected
                            ? theme.colors.surfacePressed
                            : theme.colors.surfaceHigh,
                    paddingVertical: 12,
                    paddingLeft: 16,
                    paddingRight: rightPadding,
                    width: '100%',
                    alignSelf: 'stretch',
                    flexDirection: 'row',
                    alignItems: 'center',
                })}
            >
                {/* Avatar */}
                <Pressable
                    onPress={onAvatarPress ? (e) => { e.stopPropagation(); onAvatarPress(); } : undefined}
                    disabled={!onAvatarPress}
                    style={{
                        width: 48,
                        height: 48,
                        marginRight: 12,
                    }}
                >
                    <View style={{ opacity: isOffline ? 0.4 : 1 }}>
                        {isModelAvatar(primaryAvatarId) ? (
                            <Image
                                source={{ uri: MODEL_AVATAR_IMAGES[primaryAvatarId] }}
                                style={{ width: 48, height: 48, borderRadius: 24 }}
                                contentFit="cover"
                            />
                        ) : primaryAvatarId.startsWith('vendor:') ? (
                            <Image
                                source={getVendorIconSource(persona.agent)}
                                style={{ width: 48, height: 48, borderRadius: 24 }}
                                contentFit="cover"
                            />
                        ) : (
                            <View style={{
                                width: 48,
                                height: 48,
                                borderRadius: 24,
                                backgroundColor: theme.colors.surface,
                                alignItems: 'center',
                                justifyContent: 'center',
                                overflow: 'hidden',
                            }}>
                                <Ionicons
                                    name={iconName as any}
                                    size={28}
                                    color={isOffline ? theme.colors.textSecondary : theme.colors.text}
                                    style={isOffline ? { opacity: 0.5 } : undefined}
                                />
                            </View>
                        )}
                    </View>
                    {(() => {
                        if (suppressModelBadge) return null;
                        const badgeUri = getModelBadgeUri(persona.modelId || sessionProfileId);
                        if (!badgeUri) return null;
                        return (
                            <View style={{
                                position: 'absolute',
                                bottom: -2,
                                right: -2,
                                width: 20,
                                height: 20,
                                borderRadius: 10,
                                backgroundColor: theme.colors.surfaceHigh,
                                borderWidth: 1.5,
                                borderColor: theme.colors.surfaceHigh,
                                alignItems: 'center',
                                justifyContent: 'center',
                                overflow: 'hidden',
                            }}>
                                <Image
                                    source={{ uri: badgeUri }}
                                    style={{ width: 16, height: 16, borderRadius: 8 }}
                                    contentFit="cover"
                                />
                            </View>
                        );
                    })()}
                </Pressable>

                {/* Middle: name + last message */}
                <View style={{ flex: 1, marginRight: 8 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center' }}>
                        <Text
                            style={{
                                fontSize: 16,
                                color: theme.colors.text,
                                flexShrink: 1,
                                ...Typography.default('semiBold'),
                            }}
                            numberOfLines={1}
                        >
                            {persona.name}
                        </Text>
                        {persona.isDefault && (
                            <View
                                style={{
                                    marginLeft: 8,
                                    paddingHorizontal: 6,
                                    paddingVertical: 2,
                                    borderRadius: 4,
                                    backgroundColor: theme.colors.button.primary.background,
                                }}
                            >
                                <Text
                                    style={{
                                        fontSize: 10,
                                        color: theme.colors.button.primary.tint,
                                        ...Typography.default('semiBold'),
                                    }}
                                >
                                    {t('persona.defaultLabel')}
                                </Text>
                            </View>
                        )}
                        {workspaceLabel && (
                            <View
                                style={{
                                    marginLeft: 8,
                                    paddingHorizontal: 6,
                                    paddingVertical: 2,
                                    borderRadius: 999,
                                    backgroundColor: theme.colors.surface,
                                    borderWidth: 1,
                                    borderColor: theme.colors.divider,
                                }}
                            >
                                <Text
                                    style={{
                                        fontSize: 10,
                                        color: theme.colors.textSecondary,
                                        ...Typography.default('semiBold'),
                                    }}
                                >
                                    {workspaceLabel}
                                </Text>
                            </View>
                        )}
                    </View>

                    <Text
                        style={{
                            fontSize: 13,
                            color: theme.colors.textSecondary,
                            marginTop: 3,
                            ...Typography.default(),
                        }}
                        numberOfLines={1}
                    >
                        {previewText}
                    </Text>
                </View>

                {/* Right: timestamp + icons + unread badge */}
                <View style={{ alignItems: 'flex-end', justifyContent: 'space-between', height: 42 }}>
                    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 4 }}>
                        {isThinking && (
                            <StatusDot color="#007AFF" isPulsing size={6} />
                        )}
                        {hasScheduledTask && (
                            <Ionicons name="time-outline" size={14} color={theme.colors.text} />
                        )}
                        {lastMessage ? (
                            <Text style={{
                                fontSize: 12,
                                color: theme.colors.textSecondary,
                                ...Typography.default(),
                            }}>
                                {formatRelativeTime(lastMessage.createdAt)}
                            </Text>
                        ) : hasScheduledTask ? null : <View />}
                    </View>

                    {unreadCount > 0 && (
                        <View style={{
                            minWidth: 20,
                            height: 20,
                            borderRadius: 10,
                            backgroundColor: theme.colors.status.error,
                            alignItems: 'center',
                            justifyContent: 'center',
                            paddingHorizontal: 6,
                        }}>
                            <Text style={{
                                fontSize: 12,
                                color: '#FFFFFF',
                                ...Typography.default('semiBold'),
                            }}>
                                {unreadCount > 99 ? '99+' : unreadCount}
                            </Text>
                        </View>
                    )}
                </View>
            </Pressable>

            {/* Web: absolute-positioned delete button (sibling, not nested in Pressable) */}
            {showDeleteControls && (
                <Pressable
                    accessible={false}
                    style={{
                        position: 'absolute',
                        right: 8,
                        top: 0,
                        bottom: 0,
                        flexDirection: 'row',
                        alignItems: 'center',
                        gap: 8,
                    }}
                    onHoverIn={showHover}
                    onHoverOut={scheduleHideHover}
                >
                    {confirmingDelete ? (
                        <>
                            <Pressable
                                onPress={(e) => {
                                    e.stopPropagation();
                                    setConfirmingDelete(false);
                                }}
                                onHoverIn={showHover}
                                onHoverOut={scheduleHideHover}
                                style={({ pressed }) => ({
                                    height: 30,
                                    paddingHorizontal: 10,
                                    borderRadius: 999,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: pressed ? theme.colors.surfacePressed : theme.colors.surface,
                                })}
                            >
                                <Text style={{
                                    fontSize: 12,
                                    color: theme.colors.textSecondary,
                                    ...Typography.default('semiBold'),
                                }}>
                                    {t('common.cancel')}
                                </Text>
                            </Pressable>
                            <Pressable
                                onPress={(e) => {
                                    void handleConfirmDelete(e);
                                }}
                                onHoverIn={showHover}
                                onHoverOut={scheduleHideHover}
                                disabled={deleting}
                                style={({ pressed }) => ({
                                    minWidth: 68,
                                    height: 30,
                                    paddingHorizontal: 12,
                                    borderRadius: 999,
                                    alignItems: 'center',
                                    justifyContent: 'center',
                                    backgroundColor: deleting
                                        ? theme.colors.surfacePressed
                                        : (pressed ? theme.colors.status.error : theme.colors.deleteAction),
                                })}
                            >
                                {deleting ? (
                                    <Ionicons name="hourglass-outline" size={14} color="#FFFFFF" />
                                ) : (
                                    <Text style={{
                                        fontSize: 12,
                                        color: '#FFFFFF',
                                        ...Typography.default('semiBold'),
                                    }}>
                                        {t('common.delete')}
                                    </Text>
                                )}
                            </Pressable>
                        </>
                    ) : (
                        <Pressable
                            onPressIn={(e) => e.stopPropagation()}
                            onPress={(e) => {
                                e.stopPropagation();
                                setConfirmingDelete(true);
                            }}
                            onHoverIn={showHover}
                            onHoverOut={scheduleHideHover}
                            disabled={deleting}
                            style={({ pressed }) => ({
                                width: 32,
                                height: 32,
                                borderRadius: 16,
                                alignItems: 'center',
                                justifyContent: 'center',
                                backgroundColor: pressed ? theme.colors.surfacePressed : 'transparent',
                                opacity: deleting ? 0.5 : 1,
                            })}
                        >
                            <Ionicons name="trash-outline" size={16} color={theme.colors.textSecondary} />
                        </Pressable>
                    )}
                </Pressable>
            )}
        </Pressable>
    );

    if (swipeEnabled && onDelete) {
        return (
            <Swipeable
                ref={swipeableRef}
                renderRightActions={renderRightActions}
                overshootRight={false}
            >
                {cardContent}
            </Swipeable>
        );
    }

    return cardContent;
};
