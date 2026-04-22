import * as React from 'react';
import { View, Image as RNImage, ImageStyle, StyleProp } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { Text } from '@/components/StyledText';

interface RemoteImageProps {
    uri: string;
    style?: StyleProp<ImageStyle>;
    resizeMode?: 'cover' | 'contain' | 'stretch' | 'center';
    expiredText?: string;
}

/**
 * Image component that handles load errors (e.g. expired OSS signed URLs)
 * by showing a placeholder instead of a blank area.
 */
export const RemoteImage = React.memo((props: RemoteImageProps) => {
    const { uri, style, resizeMode = 'cover', expiredText = 'Image expired' } = props;
    const [error, setError] = React.useState(false);

    if (error) {
        return (
            <View style={[style, styles.expired]}>
                <Ionicons name="image-outline" size={28} color="#999" />
                <Text style={styles.expiredText}>{expiredText}</Text>
            </View>
        );
    }

    return (
        <RNImage
            source={{ uri }}
            style={style}
            resizeMode={resizeMode}
            onError={() => setError(true)}
        />
    );
});

const styles = StyleSheet.create((theme) => ({
    expired: {
        justifyContent: 'center',
        alignItems: 'center',
        backgroundColor: theme.colors.surface,
        gap: 6,
    },
    expiredText: {
        fontSize: 12,
        color: theme.colors.textSecondary,
    },
}));
