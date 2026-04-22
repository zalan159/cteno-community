import * as React from 'react';
import { View, StyleSheet, Platform, Pressable } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Ionicons } from '@expo/vector-icons';
import { useNavigation } from '@react-navigation/native';
import { Avatar } from '@/components/Avatar';
import { Typography } from '@/constants/Typography';
import { useHeaderHeight } from '@/utils/responsive';
import { layout } from '@/components/layout';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';

interface ChatHeaderViewProps {
    title: string;
    subtitle?: string;
    onBackPress?: () => void;
    onHomePress?: () => void;
    onAvatarPress?: () => void;
    onMemoryPress?: () => void;
    onWorkspacePress?: () => void;
    onNotificationsPress?: () => void;
    notificationCount?: number;
    onContinuousBrowsingToggle?: () => void;
    continuousBrowsing?: boolean;
    onMorePress?: () => void;
    avatarId?: string;
    backgroundColor?: string;
    tintColor?: string;
    isConnected?: boolean;
    flavor?: string | null;
    rightContent?: React.ReactNode;
}

export const ChatHeaderView: React.FC<ChatHeaderViewProps> = ({
    title,
    subtitle,
    onBackPress,
    onHomePress,
    onAvatarPress,
    onMemoryPress,
    onWorkspacePress,
    onNotificationsPress,
    notificationCount,
    onContinuousBrowsingToggle,
    continuousBrowsing,
    onMorePress,
    avatarId,
    isConnected = true,
    flavor,
    rightContent,
}) => {
    const { theme } = useUnistyles();
    const navigation = useNavigation();
    const insets = useSafeAreaInsets();
    const headerHeight = useHeaderHeight();

    const handleBackPress = () => {
        if (onBackPress) {
            onBackPress();
        } else {
            navigation.goBack();
        }
    };

    return (
        <View style={[styles.container, { paddingTop: insets.top, backgroundColor: theme.colors.header.background }]}>
            <View style={styles.contentWrapper}>
                <View style={[styles.content, { height: headerHeight }]}>
                <Pressable onPress={handleBackPress} style={styles.backButton} hitSlop={15}>
                    <Ionicons
                        name={Platform.OS === 'ios' ? 'chevron-back' : 'arrow-back'}
                        size={Platform.select({ ios: 28, default: 24 })}
                        color={theme.colors.header.tint}
                    />
                </Pressable>
                {onHomePress && (
                    <Pressable onPress={onHomePress} hitSlop={15} style={{ marginRight: 4 }}>
                        <Ionicons
                            name="home-outline"
                            size={20}
                            color={theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                
                <View style={styles.titleContainer}>
                    <Text
                        numberOfLines={1}
                        ellipsizeMode="tail"
                        style={[
                            styles.title,
                            {
                                color: theme.colors.header.tint,
                                ...Typography.default('semiBold')
                            }
                        ]}
                    >
                        {title}
                    </Text>
                    {!!subtitle && (
                        <Text
                            numberOfLines={1}
                            ellipsizeMode="tail"
                            style={[
                                styles.subtitle,
                                {
                                    color: theme.colors.header.tint,
                                    opacity: 0.7,
                                    ...Typography.default()
                                }
                            ]}
                        >
                            {subtitle}
                        </Text>
                    )}
                </View>
                
                {onContinuousBrowsingToggle && (
                    <Pressable
                        onPress={onContinuousBrowsingToggle}
                        hitSlop={15}
                        style={styles.memoryButton}
                    >
                        <Ionicons
                            name={continuousBrowsing ? 'infinite' : 'infinite-outline'}
                            size={22}
                            color={continuousBrowsing ? '#34C759' : theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                {onNotificationsPress && (
                    <Pressable
                        onPress={onNotificationsPress}
                        hitSlop={15}
                        style={styles.memoryButton}
                    >
                        <Ionicons
                            name={(notificationCount ?? 0) > 0 ? 'notifications' : 'notifications-outline'}
                            size={20}
                            color={(notificationCount ?? 0) > 0 ? '#FF9500' : theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                {onMemoryPress && (
                    <Pressable
                        onPress={onMemoryPress}
                        hitSlop={15}
                        style={styles.memoryButton}
                    >
                        <Ionicons
                            name="reader-outline"
                            size={20}
                            color={theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                {onWorkspacePress && (
                    <Pressable
                        onPress={onWorkspacePress}
                        hitSlop={15}
                        style={styles.memoryButton}
                    >
                        <Ionicons
                            name="folder-open-outline"
                            size={20}
                            color={theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                {onMorePress && (
                    <Pressable
                        onPress={onMorePress}
                        hitSlop={15}
                        style={styles.memoryButton}
                    >
                        <Ionicons
                            name="ellipsis-horizontal"
                            size={22}
                            color={theme.colors.header.tint}
                        />
                    </Pressable>
                )}
                {rightContent}
                {avatarId && onAvatarPress && (
                    <Pressable
                        onPress={onAvatarPress}
                        hitSlop={15}
                        style={styles.avatarButton}
                    >
                        <Avatar
                            id={avatarId}
                            size={32}
                            monochrome={!isConnected}
                            flavor={flavor}
                        />
                    </Pressable>
                )}
                </View>
            </View>
        </View>
    );
};

const styles = StyleSheet.create({
    container: {
        position: 'relative',
        zIndex: 100,
    },
    contentWrapper: {
        width: '100%',
        alignItems: 'center',
    },
    content: {
        flexDirection: 'row',
        alignItems: 'center',
        paddingHorizontal: Platform.OS === 'ios' ? 8 : 16,
        width: '100%',
        maxWidth: layout.headerMaxWidth,
    },
    backButton: {
        marginRight: 8,
    },
    titleContainer: {
        flex: 1,
        justifyContent: 'center',
        alignItems: 'flex-start',
    },
    title: {
        fontSize: Platform.select({
            ios: 15,
            android: 15,
            default: 16
        }),
        fontWeight: '600',
        marginBottom: 1,
        width: '100%',
    },
    subtitle: {
        fontSize: 12,
        fontWeight: '400',
        lineHeight: 14,
    },
    memoryButton: {
        width: 36,
        height: 44,
        alignItems: 'center',
        justifyContent: 'center',
    },
    avatarButton: {
        width: 44,
        height: 44,
        alignItems: 'center',
        justifyContent: 'center',
        marginRight: Platform.select({ ios: -8, default: -8 }),
    },
});
