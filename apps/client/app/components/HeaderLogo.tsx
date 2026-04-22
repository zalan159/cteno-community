import * as React from 'react';
import { View, Pressable } from 'react-native';
import { Image } from 'expo-image';
import { useUnistyles } from 'react-native-unistyles';
import { Text } from '@/components/StyledText';
import { Typography } from '@/constants/Typography';
import { useTauriUpdate } from '@/hooks/useTauriUpdate';
import { storage } from '@/sync/storage';

/**
 * Shared header logo component used across all main tabs.
 * Extracted to prevent flickering on tab switches - when each tab
 * had its own HeaderLeft, the component would unmount/remount.
 */
export const HeaderLogo = React.memo(() => {
    const { theme } = useUnistyles();
    const tauriUpdate = useTauriUpdate();

    return (
        <Pressable
            style={{
                width: 32,
                height: 32,
                alignItems: 'center',
                justifyContent: 'center',
            }}
            onPress={tauriUpdate.available ? () => {
                storage.getState().setShowDesktopUpdateModal(true);
            } : undefined}
            disabled={!tauriUpdate.available}
        >
            <Image
                source={theme.colors.header.tint === '#ffffff'
                    ? require('@/assets/images/logo-white.png')
                    : require('@/assets/images/logo-black.png')}
                contentFit="contain"
                style={{ width: 24, height: 24 }}
            />
            {tauriUpdate.available && (
                <View style={{
                    position: 'absolute',
                    top: -2,
                    right: -6,
                    backgroundColor: '#FF3B30',
                    borderRadius: 7,
                    minWidth: 14,
                    height: 14,
                    paddingHorizontal: 3,
                    alignItems: 'center',
                    justifyContent: 'center',
                }}>
                    <Text style={{
                        color: '#FFFFFF',
                        fontSize: 9,
                        ...Typography.default('semiBold'),
                    }}>
                        NEW
                    </Text>
                </View>
            )}
        </Pressable>
    );
});
