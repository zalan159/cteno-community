import React, { useState, useCallback } from 'react';
import { View, Pressable, FlatList, Modal as RNModal } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';
import type { AgentNotification } from '@/sync/storageTypes';

const PRIORITY_COLORS: Record<string, string> = {
    low: '#6B7280',
    normal: '#3B82F6',
    high: '#F59E0B',
    urgent: '#EF4444',
};

function timeAgo(dateStr: string): string {
    const diff = Date.now() - new Date(dateStr).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return 'just now';
    if (mins < 60) return `${mins}m ago`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.floor(hours / 24);
    return `${days}d ago`;
}

interface NotificationCenterProps {
    notifications: AgentNotification[];
    unreadCount: number;
    onMarkRead: (id: string) => void;
}

export const NotificationCenter = React.memo(({ notifications, unreadCount, onMarkRead }: NotificationCenterProps) => {
    const { theme } = useUnistyles();
    const router = useRouter();
    const [visible, setVisible] = useState(false);

    const handlePress = useCallback((notification: AgentNotification) => {
        if (!notification.read) {
            onMarkRead(notification.id);
        }
        setVisible(false);
    }, [onMarkRead, router]);

    const renderItem = useCallback(({ item }: { item: AgentNotification }) => {
        const priorityColor = PRIORITY_COLORS[item.priority] || PRIORITY_COLORS.normal;
        return (
            <Pressable
                onPress={() => handlePress(item)}
                style={[
                    styles.notificationItem,
                    { backgroundColor: item.read ? 'transparent' : theme.colors.surfaceHighest + '40' },
                ]}
            >
                <View style={styles.notificationHeader}>
                    {!item.read && (
                        <View style={[styles.unreadDot, { backgroundColor: priorityColor }]} />
                    )}
                    <Text style={[styles.notificationTitle, { color: theme.colors.text }]} numberOfLines={1}>
                        {item.title}
                    </Text>
                    <Text style={[styles.notificationTime, { color: theme.colors.textSecondary }]}>
                        {timeAgo(item.createdAt)}
                    </Text>
                </View>
                <Text style={[styles.notificationBody, { color: theme.colors.textSecondary }]} numberOfLines={2}>
                    {item.body}
                </Text>
            </Pressable>
        );
    }, [handlePress, theme]);

    return (
        <>
            <Pressable onPress={() => setVisible(true)} style={styles.bellButton}>
                <Ionicons name="notifications-outline" size={22} color={theme.colors.text} />
                {unreadCount > 0 && (
                    <View style={styles.badge}>
                        <Text style={styles.badgeText}>
                            {unreadCount > 99 ? '99+' : unreadCount}
                        </Text>
                    </View>
                )}
            </Pressable>
            <RNModal
                visible={visible}
                transparent
                animationType="fade"
                onRequestClose={() => setVisible(false)}
            >
                <Pressable style={styles.backdrop} onPress={() => setVisible(false)}>
                    <Pressable style={[styles.modal, { backgroundColor: theme.colors.surface }]} onPress={() => {}}>
                        <View style={[styles.modalHeader, { borderBottomColor: theme.colors.divider }]}>
                            <Text style={[styles.modalTitle, { color: theme.colors.text }]}>Notifications</Text>
                            <Pressable onPress={() => setVisible(false)}>
                                <Ionicons name="close" size={22} color={theme.colors.textSecondary} />
                            </Pressable>
                        </View>
                        {notifications.length === 0 ? (
                            <View style={styles.emptyContainer}>
                                <Text style={[styles.emptyText, { color: theme.colors.textSecondary }]}>No notifications</Text>
                            </View>
                        ) : (
                            <FlatList
                                data={notifications}
                                keyExtractor={(item) => item.id}
                                renderItem={renderItem}
                                style={styles.list}
                            />
                        )}
                    </Pressable>
                </Pressable>
            </RNModal>
        </>
    );
});

const styles = StyleSheet.create((theme) => ({
    bellButton: {
        padding: 8,
        position: 'relative',
    },
    badge: {
        position: 'absolute',
        top: 4,
        right: 4,
        minWidth: 16,
        height: 16,
        borderRadius: 8,
        backgroundColor: '#EF4444',
        justifyContent: 'center',
        alignItems: 'center',
        paddingHorizontal: 4,
    },
    badgeText: {
        fontSize: 10,
        color: '#FFFFFF',
        ...Typography.default('semiBold'),
    },
    backdrop: {
        flex: 1,
        backgroundColor: 'rgba(0,0,0,0.4)',
        justifyContent: 'center',
        alignItems: 'center',
    },
    modal: {
        width: '90%',
        maxWidth: 400,
        maxHeight: '70%',
        borderRadius: 12,
        overflow: 'hidden',
    },
    modalHeader: {
        flexDirection: 'row',
        justifyContent: 'space-between',
        alignItems: 'center',
        paddingHorizontal: 16,
        paddingVertical: 12,
        borderBottomWidth: 1,
    },
    modalTitle: {
        fontSize: 17,
        ...Typography.default('semiBold'),
    },
    list: {
        flex: 1,
    },
    notificationItem: {
        paddingHorizontal: 16,
        paddingVertical: 12,
        borderBottomWidth: 0.5,
        borderBottomColor: 'rgba(128,128,128,0.2)',
    },
    notificationHeader: {
        flexDirection: 'row',
        alignItems: 'center',
        gap: 6,
        marginBottom: 4,
    },
    unreadDot: {
        width: 6,
        height: 6,
        borderRadius: 3,
    },
    notificationTitle: {
        flex: 1,
        fontSize: 14,
        ...Typography.default('semiBold'),
    },
    notificationTime: {
        fontSize: 11,
        ...Typography.default(),
    },
    notificationBody: {
        fontSize: 13,
        lineHeight: 18,
        ...Typography.default(),
    },
    emptyContainer: {
        padding: 40,
        alignItems: 'center',
    },
    emptyText: {
        fontSize: 14,
        ...Typography.default(),
    },
}));
