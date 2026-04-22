import React from 'react';
import { View, Platform, ActivityIndicator } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Typography } from '@/constants/Typography';
import { Text } from '@/components/StyledText';

interface GoalTreeWebViewProps {
    html: string | null;
}

export const GoalTreeWebView = React.memo(({ html }: GoalTreeWebViewProps) => {
    const { theme } = useUnistyles();

    if (!html) {
        return (
            <View style={[styles.container, styles.loadingContainer, { backgroundColor: theme.colors.surfaceHighest }]}>
                <ActivityIndicator size="small" color={theme.colors.textSecondary} />
                <Text style={[styles.loadingText, { color: theme.colors.textSecondary }]}>Loading goal tree...</Text>
            </View>
        );
    }

    if (Platform.OS === 'web') {
        return (
            <View style={[styles.container, { backgroundColor: theme.colors.surfaceHighest }]}>
                {/* @ts-ignore - Web only */}
                <iframe
                    srcDoc={html}
                    sandbox="allow-scripts"
                    style={{
                        width: '100%',
                        height: '100%',
                        border: 'none',
                        borderRadius: 8,
                    }}
                />
            </View>
        );
    }

    // Native: use WebView
    const WebView = require('react-native-webview').WebView;
    return (
        <View style={[styles.container, { backgroundColor: theme.colors.surfaceHighest }]}>
            <WebView
                source={{ html }}
                style={{ flex: 1 }}
                scrollEnabled={true}
            />
        </View>
    );
});

const styles = StyleSheet.create((theme) => ({
    container: {
        flex: 1,
        borderRadius: 8,
        overflow: 'hidden',
    },
    loadingContainer: {
        justifyContent: 'center',
        alignItems: 'center',
        gap: 8,
        minHeight: 200,
    },
    loadingText: {
        fontSize: 13,
        ...Typography.default(),
    },
}));
