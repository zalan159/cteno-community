import { isDesktopLocalModeEnabled } from '@/auth/local_mode';
import { getOptionalCloudSyncEnabled, getOptionalHappyServerUrl } from '@/config/runtime';
import { isServerAvailable } from '@/sync/serverConfig';

export interface RuntimeCapabilities {
    localSessions: boolean;
    accountAuth: boolean;
    cloudSync: boolean;
}

export function isCloudSyncEnabled(): boolean {
    const explicit = getOptionalCloudSyncEnabled();
    if (explicit !== null) {
        return explicit;
    }

    return getOptionalHappyServerUrl() !== null;
}

export function getRuntimeCapabilities(): RuntimeCapabilities {
    return {
        localSessions: isDesktopLocalModeEnabled(),
        accountAuth: isServerAvailable(),
        cloudSync: isCloudSyncEnabled(),
    };
}

export function shouldUseLocalTokenLogin(): boolean {
    const caps = getRuntimeCapabilities();
    return caps.localSessions && caps.accountAuth && !caps.cloudSync;
}

export function canUseCloudServerAccess(token: string | null | undefined): boolean {
    return isCloudSyncEnabled() && !!token?.trim() && isServerAvailable();
}
