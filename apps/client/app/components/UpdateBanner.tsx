import React from 'react';
import { Ionicons } from '@expo/vector-icons';
import { Item } from './Item';
import { ItemGroup } from './ItemGroup';
import { useUnistyles } from 'react-native-unistyles';
import { useUpdates } from '@/hooks/useUpdates';
import { useChangelog } from '@/hooks/useChangelog';
import { useNativeUpdate } from '@/hooks/useNativeUpdate';
import { useTauriUpdate } from '@/hooks/useTauriUpdate';
import { useRouter } from 'expo-router';
import { Linking, Platform } from 'react-native';
import { t } from '@/text';
import { storage } from '@/sync/storage';

export const UpdateBanner = React.memo(() => {
    const { theme } = useUnistyles();
    const { updateAvailable, reloadApp } = useUpdates();
    const { hasUnread, markAsRead } = useChangelog();
    const updateUrl = useNativeUpdate();
    const tauriUpdate = useTauriUpdate();
    const router = useRouter();

    // Tauri desktop update (highest priority) — open modal
    if (tauriUpdate.available) {
        return (
            <ItemGroup>
                <Item
                    title={t('updateBanner.desktopUpdateAvailable')}
                    subtitle={t('updateBanner.desktopUpdateSubtitle', { version: tauriUpdate.version ?? '' })}
                    icon={<Ionicons name="desktop-outline" size={28} color={theme.colors.success} />}
                    showChevron={true}
                    onPress={() => storage.getState().setShowDesktopUpdateModal(true)}
                />
            </ItemGroup>
        );
    }

    // Show native app update banner (second priority)
    if (updateUrl) {
        const handleOpenStore = async () => {
            try {
                const supported = await Linking.canOpenURL(updateUrl);
                if (supported) {
                    await Linking.openURL(updateUrl);
                }
            } catch (error) {
                console.error('Error opening app store:', error);
            }
        };

        return (
            <ItemGroup>
                <Item
                    title={t('updateBanner.nativeUpdateAvailable')}
                    subtitle={Platform.OS === 'ios' ? t('updateBanner.tapToUpdateAppStore') : t('updateBanner.tapToUpdatePlayStore')}
                    icon={<Ionicons name="download-outline" size={28} color={theme.colors.success} />}
                    showChevron={true}
                    onPress={handleOpenStore}
                />
            </ItemGroup>
        );
    }

    // Show OTA update banner if available (third priority)
    if (updateAvailable) {
        return (
            <ItemGroup>
                <Item
                    title={t('updateBanner.updateAvailable')}
                    subtitle={t('updateBanner.pressToApply')}
                    icon={<Ionicons name="download-outline" size={28} color={theme.colors.success} />}
                    showChevron={false}
                    onPress={reloadApp}
                />
            </ItemGroup>
        );
    }

    // Show changelog banner if there are unread changelog entries (lowest priority)
    if (hasUnread) {
        return (
            <ItemGroup>
                <Item
                    title={t('updateBanner.whatsNew')}
                    subtitle={t('updateBanner.seeLatest')}
                    icon={<Ionicons name="sparkles-outline" size={28} color={theme.colors.text} />}
                    showChevron={true}
                    onPress={() => {
                        router.push('/changelog');
                        setTimeout(() => {
                            markAsRead();
                        }, 1000);
                    }}
                />
            </ItemGroup>
        );
    }

    return null;
});
