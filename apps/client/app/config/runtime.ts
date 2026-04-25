function getOptionalEnv(name: string): string | null {
    const value = process.env[name];
    if (!value || !value.trim()) {
        return null;
    }
    return value.trim();
}

export function getOptionalHappyServerUrl(): string | null {
    return getOptionalEnv('EXPO_PUBLIC_HAPPY_SERVER_URL');
}

export function getOptionalCloudSyncEnabled(): boolean | null {
    const value = getOptionalEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED');
    if (value === null) {
        return null;
    }

    const normalized = value.toLowerCase();
    if (['1', 'true', 'yes', 'on'].includes(normalized)) {
        return true;
    }
    if (['0', 'false', 'no', 'off'].includes(normalized)) {
        return false;
    }

    console.warn(`[runtime] Ignoring invalid EXPO_PUBLIC_CLOUD_SYNC_ENABLED=${value}`);
    return null;
}
