import { useCallback, useEffect, useMemo, useState } from 'react';
import { Platform } from 'react-native';
import { Ionicons } from '@expo/vector-icons';
import { Item } from '@/components/Item';
import { ItemGroup } from '@/components/ItemGroup';
import { ItemList } from '@/components/ItemList';
import { useSettingMutable, useLocalSettingMutable } from '@/sync/storage';
import { Switch } from '@/components/Switch';
import { t } from '@/text';
import { Modal } from '@/modal';
import { isTauri } from '@/utils/tauri';

type CtenoctlInstallStatus = {
    supported: boolean;
    installed: boolean;
    symlink_path: string;
    target_path: string;
    in_path: boolean;
    path_hint?: string | null;
};

function errorMessage(error: unknown): string {
    if (error instanceof Error) {
        return error.message;
    }
    if (typeof error === 'string') {
        return error;
    }
    return String(error);
}

export default function FeaturesSettingsScreen() {
    const [experiments, setExperiments] = useSettingMutable('experiments');
    const [agentInputEnterToSend, setAgentInputEnterToSend] = useSettingMutable('agentInputEnterToSend');
    const [commandPaletteEnabled, setCommandPaletteEnabled] = useLocalSettingMutable('commandPaletteEnabled');
    const [markdownCopyV2, setMarkdownCopyV2] = useLocalSettingMutable('markdownCopyV2');
    const [hideInactiveSessions, setHideInactiveSessions] = useSettingMutable('hideInactiveSessions');
    const [useEnhancedSessionWizard, setUseEnhancedSessionWizard] = useSettingMutable('useEnhancedSessionWizard');
    const desktopRuntime = isTauri();
    const [ctenoctlStatus, setCtenoctlStatus] = useState<CtenoctlInstallStatus | null>(null);
    const [ctenoctlStatusError, setCtenoctlStatusError] = useState<string | null>(null);
    const [ctenoctlStatusLoading, setCtenoctlStatusLoading] = useState(false);
    const [ctenoctlInstalling, setCtenoctlInstalling] = useState(false);

    const refreshCtenoctlStatus = useCallback(async () => {
        if (!desktopRuntime) {
            return;
        }
        setCtenoctlStatusLoading(true);
        setCtenoctlStatusError(null);
        try {
            const { invoke } = await import('@tauri-apps/api/core');
            const status = await invoke('get_ctenoctl_install_status');
            setCtenoctlStatus(status as CtenoctlInstallStatus);
        } catch (error) {
            setCtenoctlStatusError(errorMessage(error));
        } finally {
            setCtenoctlStatusLoading(false);
        }
    }, [desktopRuntime]);

    useEffect(() => {
        void refreshCtenoctlStatus();
    }, [refreshCtenoctlStatus]);

    const ctenoctlSubtitle = useMemo(() => {
        if (ctenoctlStatusError) {
            return t('settingsFeatures.ctenoctlStatusFailed', { error: ctenoctlStatusError });
        }
        if (!ctenoctlStatus) {
            return t('settingsFeatures.ctenoctlExternalToolsSubtitle');
        }
        if (!ctenoctlStatus.supported) {
            return t('settingsFeatures.ctenoctlUnsupported');
        }
        if (ctenoctlStatus.installed && ctenoctlStatus.in_path) {
            return t('settingsFeatures.ctenoctlInstalled', { path: ctenoctlStatus.symlink_path });
        }
        if (ctenoctlStatus.installed) {
            return ctenoctlStatus.path_hint
                ? t('settingsFeatures.ctenoctlNeedsPath', { hint: ctenoctlStatus.path_hint })
                : t('settingsFeatures.ctenoctlInstalledNeedsRestart', { path: ctenoctlStatus.symlink_path });
        }
        return t('settingsFeatures.ctenoctlExternalToolsSubtitle');
    }, [ctenoctlStatus, ctenoctlStatusError]);

    const installCtenoctl = useCallback(async () => {
        if (!desktopRuntime || ctenoctlInstalling || ctenoctlStatusLoading) {
            return;
        }
        setCtenoctlInstalling(true);
        setCtenoctlStatusError(null);
        try {
            const { invoke } = await import('@tauri-apps/api/core');
            const status = await invoke('install_ctenoctl');
            setCtenoctlStatus(status as CtenoctlInstallStatus);
            Modal.alert(
                t('common.success'),
                t('settingsFeatures.ctenoctlInstallSuccess')
            );
        } catch (error) {
            const message = errorMessage(error);
            setCtenoctlStatusError(message);
            Modal.alert(t('common.error'), message);
        } finally {
            setCtenoctlInstalling(false);
        }
    }, [ctenoctlInstalling, ctenoctlStatusLoading, desktopRuntime]);

    return (
        <ItemList style={{ paddingTop: 0 }}>
            {/* Experimental Features */}
            <ItemGroup 
                title={t('settingsFeatures.experiments')}
                footer={t('settingsFeatures.experimentsDescription')}
            >
                <Item
                    title={t('settingsFeatures.experimentalFeatures')}
                    subtitle={experiments ? t('settingsFeatures.experimentalFeaturesEnabled') : t('settingsFeatures.experimentalFeaturesDisabled')}
                    icon={<Ionicons name="flask-outline" size={29} color="#5856D6" />}
                    rightElement={
                        <Switch
                            value={experiments}
                            onValueChange={setExperiments}
                        />
                    }
                    showChevron={false}
                />
                <Item
                    title={t('settingsFeatures.markdownCopyV2')}
                    subtitle={t('settingsFeatures.markdownCopyV2Subtitle')}
                    icon={<Ionicons name="text-outline" size={29} color="#34C759" />}
                    rightElement={
                        <Switch
                            value={markdownCopyV2}
                            onValueChange={setMarkdownCopyV2}
                        />
                    }
                    showChevron={false}
                />
                <Item
                    title={t('settingsFeatures.hideInactiveSessions')}
                    subtitle={t('settingsFeatures.hideInactiveSessionsSubtitle')}
                    icon={<Ionicons name="eye-off-outline" size={29} color="#FF9500" />}
                    rightElement={
                        <Switch
                            value={hideInactiveSessions}
                            onValueChange={setHideInactiveSessions}
                        />
                    }
                    showChevron={false}
                />
                <Item
                    title={t('settingsFeatures.enhancedSessionWizard')}
                    subtitle={useEnhancedSessionWizard
                        ? t('settingsFeatures.enhancedSessionWizardEnabled')
                        : t('settingsFeatures.enhancedSessionWizardDisabled')}
                    icon={<Ionicons name="sparkles-outline" size={29} color="#AF52DE" />}
                    rightElement={
                        <Switch
                            value={useEnhancedSessionWizard}
                            onValueChange={setUseEnhancedSessionWizard}
                        />
                    }
                    showChevron={false}
                />
            </ItemGroup>

            {desktopRuntime && (
                <ItemGroup
                    title={t('settingsFeatures.desktopTools')}
                    footer={t('settingsFeatures.desktopToolsDescription')}
                >
                    <Item
                        title={t('settingsFeatures.ctenoctlExternalTools')}
                        subtitle={ctenoctlSubtitle}
                        subtitleLines={0}
                        detail={ctenoctlStatus?.installed
                            ? t('settingsFeatures.ctenoctlUpdate')
                            : t('settingsFeatures.ctenoctlInstall')}
                        icon={<Ionicons name="terminal-outline" size={29} color="#007AFF" />}
                        onPress={installCtenoctl}
                        loading={ctenoctlStatusLoading || ctenoctlInstalling}
                        disabled={ctenoctlStatus?.supported === false}
                        showChevron={false}
                    />
                </ItemGroup>
            )}

            {/* Web-only Features */}
            {Platform.OS === 'web' && (
                <ItemGroup 
                    title={t('settingsFeatures.webFeatures')}
                    footer={t('settingsFeatures.webFeaturesDescription')}
                >
                    <Item
                        title={t('settingsFeatures.enterToSend')}
                        subtitle={agentInputEnterToSend ? t('settingsFeatures.enterToSendEnabled') : t('settingsFeatures.enterToSendDisabled')}
                        icon={<Ionicons name="return-down-forward-outline" size={29} color="#007AFF" />}
                        rightElement={
                            <Switch
                                value={agentInputEnterToSend}
                                onValueChange={setAgentInputEnterToSend}
                            />
                        }
                        showChevron={false}
                    />
                    <Item
                        title={t('settingsFeatures.commandPalette')}
                        subtitle={commandPaletteEnabled ? t('settingsFeatures.commandPaletteEnabled') : t('settingsFeatures.commandPaletteDisabled')}
                        icon={<Ionicons name="keypad-outline" size={29} color="#007AFF" />}
                        rightElement={
                            <Switch
                                value={commandPaletteEnabled}
                                onValueChange={setCommandPaletteEnabled}
                            />
                        }
                        showChevron={false}
                    />
                </ItemGroup>
            )}
        </ItemList>
    );
}
