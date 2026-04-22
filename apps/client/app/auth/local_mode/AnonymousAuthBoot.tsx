import React from 'react';
import { View, ActivityIndicator } from 'react-native';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { StyleSheet } from 'react-native-unistyles';

const stylesheet = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        alignItems: 'center',
        justifyContent: 'center',
        backgroundColor: theme.colors.groupped.background,
        padding: 24,
    },
    label: {
        marginTop: 16,
        fontSize: 14,
        color: theme.colors.textSecondary,
        textAlign: 'center',
        ...Typography.default(),
    },
}));

/**
 * Transparent loading state used while the desktop app provisions an
 * anonymous local identity before the main sidebar / navigator mounts.
 *
 * In practice the root layout already kicks off `syncInitLocalMode` before
 * rendering any children, so this component is almost always mounted for
 * only a few frames. It exists so that:
 *   1. Logged-out desktop startup never flashes the browser login UI
 *   2. There is a single, obvious place to evolve "first time anonymous
 *      bootstrap" UX (e.g. key generation progress) if that becomes user
 *      visible.
 */
export function AnonymousAuthBoot({ message }: { message?: string }) {
    const styles = stylesheet;
    return (
        <View style={styles.container}>
            <ActivityIndicator size="small" />
            <Text style={styles.label}>{message ?? 'Preparing local workspace…'}</Text>
        </View>
    );
}
