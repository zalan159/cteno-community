import * as React from 'react';
import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';

export function HostToolBadge() {
    const { theme } = useUnistyles();

    return (
        <View
            style={[
                styles.badge,
                {
                    backgroundColor: `${theme.colors.button.primary.background}22`,
                    borderColor: `${theme.colors.button.primary.background}55`,
                },
            ]}
        >
            <Text style={[styles.badgeText, { color: theme.colors.button.primary.background }]}>
                Host Tool
            </Text>
        </View>
    );
}

const styles = StyleSheet.create(() => ({
    badge: {
        borderRadius: 999,
        borderWidth: 1,
        paddingHorizontal: 8,
        paddingVertical: 3,
        flexShrink: 0,
    },
    badgeText: {
        fontSize: 11,
        fontWeight: '600',
        textTransform: 'uppercase',
    },
}));
