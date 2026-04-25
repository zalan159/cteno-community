import React, { useState } from 'react';
import { View } from 'react-native';
import { useRouter } from 'expo-router';
import { useAuth } from '@/auth/AuthContext';
import { Ionicons } from '@expo/vector-icons';
import { Item } from '@/components/Item';
import { ItemGroup } from '@/components/ItemGroup';
import { ItemList } from '@/components/ItemList';
import { Modal } from '@/modal';
import { t } from '@/text';
import { useSettingMutable, useProfile } from '@/sync/storage';
import { sync } from '@/sync/sync';
import { useUnistyles } from 'react-native-unistyles';
import { Switch } from '@/components/Switch';
import { getDisplayName, getAvatarUrl } from '@/sync/profile';
import { Image } from 'expo-image';
import { useHappyAction } from '@/hooks/useHappyAction';
import { disconnectGitHub } from '@/sync/apiGithub';
import { disconnectService } from '@/sync/apiServices';
import { Text } from '@/components/StyledText';
import { deleteAccount } from '@/sync/apiAccount';
import { getWechatOAuthParams, disconnectWechat } from '@/sync/apiWechat';
import { openExternalUrl } from '@/utils/openExternalUrl';
import { getHostedConsoleUrl, isHostedCloudConfigured } from '@/sync/serverConfig';
import { isCloudSyncEnabled } from '@/config/capabilities';

export default React.memo(() => {
    const { theme } = useUnistyles();
    const router = useRouter();
    const auth = useAuth();
    const [analyticsOptOut, setAnalyticsOptOut] = useSettingMutable('analyticsOptOut');
    const profile = useProfile();
    const hasSignedInAccess = !!auth.credentials?.token?.trim();
    const cloudSyncEnabled = isCloudSyncEnabled();
    const hostedCloudAvailable = hasSignedInAccess && cloudSyncEnabled && isHostedCloudConfigured();

    // Profile display values
    const displayName = getDisplayName(profile);
    const githubUsername = profile.github?.login;

    // WeChat connection status / actions (dual-platform: browser QR + mobile SDK)
    const isWechatConnected = !!profile.wechat;
    const wechatNickname = profile.wechat?.nickname;
    const [connectingWechat, connectWechat] = useHappyAction(async () => {
        const params = await getWechatOAuthParams(auth.credentials!);
        await openExternalUrl(params.url);
    });
    const [disconnectingWechat, handleDisconnectWechat] = useHappyAction(async () => {
        const service = t('settings.wechat');
        const confirmed = await Modal.confirm(
            t('modals.disconnectService', { service }),
            t('modals.disconnectServiceConfirm', { service }),
            { confirmText: t('modals.disconnect'), destructive: true }
        );
        if (confirmed) {
            await disconnectWechat(auth.credentials!);
            await sync.refreshProfile();
        }
    });

    // GitHub disconnection
    const [disconnecting, handleDisconnectGitHub] = useHappyAction(async () => {
        const confirmed = await Modal.confirm(
            t('modals.disconnectGithub'),
            t('modals.disconnectGithubConfirm'),
            { confirmText: t('modals.disconnect'), destructive: true }
        );
        if (confirmed) {
            await disconnectGitHub(auth.credentials!);
        }
    });

    // Service disconnection
    const [disconnectingService, setDisconnectingService] = useState<string | null>(null);
    const handleDisconnectService = async (service: string, displayName: string) => {
        const confirmed = await Modal.confirm(
            t('modals.disconnectService', { service: displayName }),
            t('modals.disconnectServiceConfirm', { service: displayName }),
            { confirmText: t('modals.disconnect'), destructive: true }
        );
        if (confirmed) {
            setDisconnectingService(service);
            try {
                await disconnectService(auth.credentials!, service);
                await sync.refreshProfile();
                // The profile will be updated via sync
            } catch (error) {
                Modal.alert(t('common.error'), t('errors.disconnectServiceFailed', { service: displayName }));
            } finally {
                setDisconnectingService(null);
            }
        }
    };

    const handleLogout = async () => {
        const confirmed = await Modal.confirm(
            t('common.logout'),
            t('settingsAccount.logoutConfirm'),
            { confirmText: t('common.logout'), destructive: true }
        );
        if (confirmed) {
            auth.logout();
        }
    };

    const [deletingAccount, handleDeleteAccount] = useHappyAction(async () => {
        const confirmed = await Modal.confirm(
            t('settingsAccount.deleteAccount'),
            t('settingsAccount.deleteAccountConfirm'),
            { confirmText: t('settingsAccount.deleteAccount'), destructive: true }
        );
        if (!confirmed) return;

        // Double confirm for destructive action
        const doubleConfirmed = await Modal.confirm(
            t('settingsAccount.deleteAccountFinalTitle'),
            t('settingsAccount.deleteAccountFinalConfirm'),
            { confirmText: t('settingsAccount.deleteAccountFinalButton'), destructive: true }
        );
        if (!doubleConfirmed) return;

        await deleteAccount(auth.credentials!);
        auth.logout();
    });

    if (!hasSignedInAccess) {
        return (
            <ItemList>
                <ItemGroup title={t('settingsAccount.accountInformation')}>
                    <Item
                        title={t('settingsAccount.status')}
                        detail="Local Mode"
                        showChevron={false}
                    />
                    <Item
                        title="Access"
                        detail="Local-only"
                        showChevron={false}
                    />
                </ItemGroup>
                <ItemGroup footer={cloudSyncEnabled
                    ? "登录后可启用云端同步、多端账号、已连接服务等功能。本地模式不依赖账号。"
                    : "登录后仅用于 Cteno agent 内置模型鉴权，本地模式不会启用云同步。"}
                >
                    <Item
                        title="登录 / 注册 Cteno 账号"
                        subtitle="邮箱 / Google / Apple / WeChat"
                        icon={<Ionicons name="log-in-outline" size={29} color="#007AFF" />}
                        onPress={() => router.push('/login')}
                    />
                </ItemGroup>
                <ItemGroup
                    title={t('settingsAccount.privacy')}
                    footer={t('settingsAccount.privacyDescription')}
                >
                    <Item
                        title={t('settingsAccount.analytics')}
                        subtitle={analyticsOptOut ? t('settingsAccount.analyticsDisabled') : t('settingsAccount.analyticsEnabled')}
                        rightElement={
                            <Switch
                                value={!analyticsOptOut}
                                onValueChange={(value) => {
                                    const optOut = !value;
                                    setAnalyticsOptOut(optOut);
                                }}
                                trackColor={{ false: '#767577', true: '#34C759' }}
                                thumbColor="#FFFFFF"
                            />
                        }
                        showChevron={false}
                    />
                </ItemGroup>
                {/* Danger zone — greyed out; actions only make sense once signed in. */}
                <ItemGroup title={t('settingsAccount.dangerZone')} footer="登录账号后方可使用这些操作。">
                    <View pointerEvents="none" style={{ opacity: 0.4 }}>
                        <Item
                            title={t('settingsAccount.logout')}
                            subtitle={t('settingsAccount.logoutSubtitle')}
                            icon={<Ionicons name="log-out-outline" size={29} color="#FF3B30" />}
                            showChevron={false}
                        />
                        <Item
                            title={t('settingsAccount.deleteAccount')}
                            subtitle={t('settingsAccount.deleteAccountSubtitle')}
                            icon={<Ionicons name="trash-outline" size={29} color="#FF3B30" />}
                            showChevron={false}
                        />
                    </View>
                </ItemGroup>
            </ItemList>
        );
    }

    return (
        <>
            <ItemList>
                {/* Account Info */}
                <ItemGroup title={t('settingsAccount.accountInformation')}>
                    <Item
                        title="Email"
                        detail={profile.github?.email || t('settingsAccount.notAvailable')}
                        showChevron={false}
                        copy={profile.github?.email || false}
                    />
                    {hostedCloudAvailable && (
                        <Item
                            title="Hosted Cloud"
                            subtitle="在网页查看官方云服务账户与用量"
                            icon={<Ionicons name="open-outline" size={29} color="#34C759" />}
                            onPress={() => {
                                void openExternalUrl(getHostedConsoleUrl());
                            }}
                            showChevron={false}
                        />
                    )}
                </ItemGroup>

                {/* Profile Section */}
                {(displayName || githubUsername || profile.avatar) && (
                    <ItemGroup title={t('settingsAccount.profile')}>
                        {displayName && (
                            <Item
                                title={t('settingsAccount.name')}
                                detail={displayName}
                                showChevron={false}
                            />
                        )}
                        {githubUsername && (
                            <Item
                                title={t('settingsAccount.github')}
                                detail={`@${githubUsername}`}
                                subtitle={t('settingsAccount.tapToDisconnect')}
                                onPress={handleDisconnectGitHub}
                                loading={disconnecting}
                                showChevron={false}
                                icon={profile.avatar?.url ? (
                                    <Image
                                        source={{ uri: profile.avatar.url }}
                                        style={{ width: 29, height: 29, borderRadius: 14.5 }}
                                        placeholder={{ thumbhash: profile.avatar.thumbhash }}
                                        contentFit="cover"
                                        transition={200}
                                        cachePolicy="memory-disk"
                                    />
                                ) : (
                                    <Ionicons name="logo-github" size={29} color={theme.colors.textSecondary} />
                                )}
                            />
                        )}
                    </ItemGroup>
                )}

                {/* Connected Accounts Section — WeChat is always present (connect/disconnect
                    inline); Claude/Gemini/OpenAI show up when already linked. */}
                {(() => {
                    const knownServices = {
                        anthropic: { name: 'Claude Code', icon: require('@/assets/images/icon-claude.png'), tintColor: null },
                        gemini: { name: 'Google Gemini', icon: require('@/assets/images/icon-gemini.png'), tintColor: null },
                        openai: { name: 'OpenAI Codex', icon: require('@/assets/images/icon-codex.png'), tintColor: null }
                    };
                    const displayServices = (profile.connectedServices || []).filter(
                        service => service in knownServices
                    );

                    return (
                        <ItemGroup title={t('settings.connectedAccounts')}>
                            <Item
                                title={t('settings.wechat')}
                                subtitle={isWechatConnected ? wechatNickname : t('settings.connectWechatAccount')}
                                icon={
                                    <Ionicons
                                        name="chatbubble-ellipses"
                                        size={29}
                                        color={isWechatConnected ? '#07C160' : theme.colors.textSecondary}
                                    />
                                }
                                onPress={isWechatConnected ? handleDisconnectWechat : connectWechat}
                                loading={connectingWechat || disconnectingWechat}
                                showChevron={false}
                            />
                            {displayServices.map(service => {
                                const serviceInfo = knownServices[service as keyof typeof knownServices];
                                const isDisconnecting = disconnectingService === service;
                                return (
                                    <Item
                                        key={service}
                                        title={serviceInfo.name}
                                        detail={t('settingsAccount.statusActive')}
                                        subtitle={t('settingsAccount.tapToDisconnect')}
                                        onPress={() => handleDisconnectService(service, serviceInfo.name)}
                                        loading={isDisconnecting}
                                        disabled={isDisconnecting}
                                        showChevron={false}
                                        icon={
                                            <Image
                                                source={serviceInfo.icon}
                                                style={{ width: 29, height: 29 }}
                                                tintColor={serviceInfo.tintColor}
                                                contentFit="contain"
                                            />
                                        }
                                    />
                                );
                            })}
                        </ItemGroup>
                    );
                })()}

                {/* Analytics Section */}
                <ItemGroup
                    title={t('settingsAccount.privacy')}
                    footer={t('settingsAccount.privacyDescription')}
                >
                    <Item
                        title={t('settingsAccount.analytics')}
                        subtitle={analyticsOptOut ? t('settingsAccount.analyticsDisabled') : t('settingsAccount.analyticsEnabled')}
                        rightElement={
                            <Switch
                                value={!analyticsOptOut}
                                onValueChange={(value) => {
                                    const optOut = !value;
                                    setAnalyticsOptOut(optOut);
                                }}
                                trackColor={{ false: '#767577', true: '#34C759' }}
                                thumbColor="#FFFFFF"
                            />
                        }
                        showChevron={false}
                    />
                </ItemGroup>

                {/* Danger Zone */}
                <ItemGroup title={t('settingsAccount.dangerZone')}>
                    <Item
                        title={t('settingsAccount.logout')}
                        subtitle={t('settingsAccount.logoutSubtitle')}
                        icon={<Ionicons name="log-out-outline" size={29} color="#FF3B30" />}
                        destructive
                        onPress={handleLogout}
                    />
                    <Item
                        title={t('settingsAccount.deleteAccount')}
                        subtitle={t('settingsAccount.deleteAccountSubtitle')}
                        icon={<Ionicons name="trash-outline" size={29} color="#FF3B30" />}
                        destructive
                        onPress={handleDeleteAccount}
                        loading={deletingAccount}
                    />
                </ItemGroup>
            </ItemList>

        </>
    );
});
