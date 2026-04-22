import { View, ScrollView, Pressable, Platform, Linking } from 'react-native';
import { Image } from 'expo-image';
import * as React from 'react';
import { useState, useEffect } from 'react';
import { Text } from '@/components/StyledText';
import { useRouter } from 'expo-router';
import { Ionicons } from '@expo/vector-icons';
import Constants from 'expo-constants';
import { useAuth } from '@/auth/AuthContext';
import { isTauri, isMacOS } from '@/utils/tauri';
import { Typography } from "@/constants/Typography";
import { Item } from '@/components/Item';
import { ItemGroup } from '@/components/ItemGroup';
import { ItemList } from '@/components/ItemList';
import { useLocalSettingMutable, useSetting } from '@/sync/storage';
import { trackWhatsNewClicked } from '@/track';
import { Modal } from '@/modal';
import { useMultiClick } from '@/hooks/useMultiClick';
import { useAllMachines } from '@/sync/storage';
import { isMachineOnline } from '@/utils/machineUtils';
import { useUnistyles } from 'react-native-unistyles';
import { layout } from '@/components/layout';
import { useProfile } from '@/sync/storage';
import { getDisplayName, getAvatarUrl, getBio } from '@/sync/profile';
import { Avatar } from '@/components/Avatar';
import { sync } from '@/sync/sync';
import { t } from '@/text';
import { useBalanceStatus } from '@/hooks/useBalanceStatus';
import { openExternalUrl } from '@/utils/openExternalUrl';
import { getHostedConsoleUrl, isHostedCloudConfigured } from '@/sync/serverConfig';

function openHostedConsole() {
    void openExternalUrl(getHostedConsoleUrl());
}

export const SettingsView = React.memo(function SettingsView() {
    const { theme } = useUnistyles();
    const router = useRouter();
    const expoVersion = Constants.expoConfig?.version || '1.0.0';
    const [tauriVersion, setTauriVersion] = useState<string | null>(null);
    const appVersion = tauriVersion || expoVersion;

    useEffect(() => {
        if (isTauri()) {
            import('@tauri-apps/api/app').then(({ getVersion }) =>
                getVersion().then(setTauriVersion).catch(() => {})
            );
        }
    }, []);

    const auth = useAuth();
    const hasSignedInAccess = !!auth.credentials?.token?.trim();
    const [devModeEnabled, setDevModeEnabled] = useLocalSettingMutable('devModeEnabled');
    const experiments = useSetting('experiments');
    const allMachines = useAllMachines();
    const profile = useProfile();
    const displayName = getDisplayName(profile);
    const avatarUrl = getAvatarUrl(profile);
    const bio = getBio(profile);
    const { balance } = useBalanceStatus();
    const hostedCloudAvailable = hasSignedInAccess && isHostedCloudConfigured();

    // Use the multi-click hook for version clicks
    const handleVersionClick = useMultiClick(() => {
        // Toggle dev mode
        const newDevMode = !devModeEnabled;
        setDevModeEnabled(newDevMode);
        Modal.alert(
            t('modals.developerMode'),
            newDevMode ? t('modals.developerModeEnabled') : t('modals.developerModeDisabled')
        );
    }, {
        requiredClicks: 10,
        resetTimeout: 2000
    });


    return (
    <>
        <ItemList style={{ paddingTop: 0 }}>
            {/* App Info Header */}
            <View style={{ maxWidth: layout.maxWidth, alignSelf: 'center', width: '100%' }}>
                <View style={{ alignItems: 'center', paddingVertical: 24, backgroundColor: theme.colors.surface, marginTop: 16, borderRadius: 12, marginHorizontal: 16 }}>
                    {hasSignedInAccess ? (
                        // Profile view: Avatar left + name & balance right
                        <View style={{ flexDirection: 'row', alignItems: 'center', width: '100%', paddingHorizontal: 20 }}>
                            <Avatar
                                id={profile.id}
                                size={72}
                                imageUrl={avatarUrl}
                                thumbhash={profile.avatar?.thumbhash}
                            />
                            <View style={{ flex: 1, marginLeft: 16 }}>
                                <Text style={{ fontSize: 20, fontWeight: '600', color: theme.colors.text }} numberOfLines={1}>
                                    {displayName || profile.email || '已登录'}
                                </Text>
                                {!!bio && (
                                    <Text style={{ fontSize: 14, color: theme.colors.textSecondary, marginTop: 2 }} numberOfLines={1}>
                                        {bio}
                                    </Text>
                                )}
                                {hostedCloudAvailable && (balance?.balanceYuan ?? 0) > 0 && (
                                    <Pressable
                                        onPress={openHostedConsole}
                                        style={{ flexDirection: 'row', alignItems: 'center', marginTop: 8, backgroundColor: theme.colors.surfaceHigh, borderRadius: 8, paddingHorizontal: 10, paddingVertical: 5, alignSelf: 'flex-start' }}
                                    >
                                        <Ionicons name="wallet-outline" size={14} color={theme.colors.success} />
                                        <Text style={{ fontSize: 14, fontWeight: '600', color: theme.colors.text, marginLeft: 4 }}>
                                            ¥{(balance?.balanceYuan ?? 0).toFixed(2)}
                                        </Text>
                                        <Ionicons name="chevron-forward" size={12} color={theme.colors.textSecondary} style={{ marginLeft: 2 }} />
                                    </Pressable>
                                )}
                            </View>
                        </View>
                    ) : (
                        // Logo view: Original logo + version
                        <>
                            <Image
                                source={theme.dark ? require('@/assets/images/logotype-light.png') : require('@/assets/images/logotype-dark.png')}
                                contentFit="contain"
                                style={{ width: 300, height: 120, marginBottom: 12 }}
                            />
                        </>
                    )}
                </View>
            </View>

            {/* Social */}
            {/* <ItemGroup title={t('settings.social')}>
                <Item
                    title={t('navigation.friends')}
                    subtitle={t('friends.manageFriends')}
                    icon={<Ionicons name="people-outline" size={29} color="#007AFF" />}
                    onPress={() => router.push('/friends')}
                />
            </ItemGroup> */}

            {/* Machines (sorted: online first, then last seen desc) */}
            {hasSignedInAccess && allMachines.length > 0 && (
                <ItemGroup title={t('settings.machines')}>
                    {[...allMachines].map((machine) => {
                        // Handle decryption failed case
                        if (machine.decryptionFailed) {
                            return (
                                <Item
                                    key={machine.id}
                                    title="🔐 需要导入设备密钥"
                                    subtitle="在其他在线设备上导出密钥，或通过扫码添加"
                                    icon={
                                        <Ionicons
                                            name="lock-closed"
                                            size={29}
                                            color={theme.colors.warning || '#FF9500'}
                                        />
                                    }
                                    onPress={() => router.push(`/machine/${machine.id}`)}
                                />
                            );
                        }

                        const isOnline = isMachineOnline(machine);
                        const host = machine.metadata?.host || t('status.unknown');
                        const displayName = machine.metadata?.displayName;
                        const platform = machine.metadata?.platform || '';

                        // Use displayName if available, otherwise use host
                        const title = displayName || host;

                        // Build subtitle: show hostname if different from title, plus platform and status
                        let subtitle = '';
                        if (displayName && displayName !== host) {
                            subtitle = host;
                        }
                        if (platform) {
                            subtitle = subtitle ? `${subtitle} • ${platform}` : platform;
                        }
                        subtitle = subtitle ? `${subtitle} • ${isOnline ? t('status.online') : t('status.offline')}` : (isOnline ? t('status.online') : t('status.offline'));

                        return (
                            <Item
                                key={machine.id}
                                title={title}
                                subtitle={subtitle}
                                icon={
                                    <Ionicons
                                        name="desktop-outline"
                                        size={29}
                                        color={isOnline ? theme.colors.status.connected : theme.colors.status.disconnected}
                                    />
                                }
                                onPress={() => router.push(`/machine/${machine.id}`)}
                            />
                        );
                    })}
                </ItemGroup>
            )}

            {/* Features */}
            <ItemGroup title={t('settings.features')}>
                {hostedCloudAvailable && (
                    <Item
                        title="云端账户"
                        subtitle="前往网页查看官方云服务账户与用量"
                        icon={<Ionicons name="wallet-outline" size={29} color="#34C759" />}
                        onPress={openHostedConsole}
                    />
                )}
                <Item
                    title={t('settings.account')}
                    subtitle={hasSignedInAccess ? t('settings.accountSubtitle') : '未登录 — 进入管理登录、已连接账号与敏感操作'}
                    icon={<Ionicons name="person-circle-outline" size={29} color="#007AFF" />}
                    onPress={() => router.push('/settings/account')}
                />
                <Item
                    title={t('settings.appearance')}
                    subtitle={t('settings.appearanceSubtitle')}
                    icon={<Ionicons name="color-palette-outline" size={29} color="#5856D6" />}
                    onPress={() => router.push('/settings/appearance')}
                />
                <Item
                    title={t('settings.featuresTitle')}
                    subtitle={t('settings.featuresSubtitle')}
                    icon={<Ionicons name="flask-outline" size={29} color="#FF9500" />}
                    onPress={() => router.push('/settings/features')}
                />
                <Item
                    title={t('settings.usage')}
                    subtitle={t('settings.usageSubtitle')}
                    icon={<Ionicons name="analytics-outline" size={29} color="#007AFF" />}
                    onPress={() => router.push('/settings/usage')}
                />
            </ItemGroup>

            {/* Developer */}
            {(__DEV__ || devModeEnabled) && (
                <ItemGroup title={t('settings.developer')}>
                    <Item
                        title={t('settings.developerTools')}
                        icon={<Ionicons name="construct-outline" size={29} color="#5856D6" />}
                        onPress={() => router.push('/dev')}
                    />
                </ItemGroup>
            )}

            {/* About */}
            <ItemGroup title={t('settings.about')} footer={t('settings.aboutFooter')}>
                <Item
                    title={t('settings.whatsNew')}
                    subtitle={t('settings.whatsNewSubtitle')}
                    icon={<Ionicons name="sparkles-outline" size={29} color="#FF9500" />}
                    onPress={() => {
                        trackWhatsNewClicked();
                        router.push('/changelog');
                    }}
                />
                {Platform.OS === 'ios' && (
                    <Item
                        title={t('settings.eula')}
                        icon={<Ionicons name="document-text-outline" size={29} color="#007AFF" />}
                        onPress={async () => {
                            const url = 'https://www.apple.com/legal/internet-services/itunes/dev/stdeula/';
                            const supported = await Linking.canOpenURL(url);
                            if (supported) {
                                await Linking.openURL(url);
                            }
                        }}
                    />
                )}
                <Item
                    title={t('common.version')}
                    detail={appVersion}
                    icon={<Ionicons name="information-circle-outline" size={29} color={theme.colors.textSecondary} />}
                    onPress={handleVersionClick}
                    showChevron={false}
                />
                {Platform.OS !== 'web' && (
                    <Item
                        title={t('settings.icpBeian')}
                        detail="粤ICP备2023043025号-4A"
                        icon={<Ionicons name="shield-checkmark-outline" size={29} color="#34C759" />}
                        showChevron={false}
                    />
                )}
            </ItemGroup>

        </ItemList>
    </>
    );
});
