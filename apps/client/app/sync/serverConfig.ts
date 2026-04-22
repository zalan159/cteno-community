import { MMKV } from 'react-native-mmkv';
import { getDefaultServerUrl } from '@/config/serverDefaults';

// Separate MMKV instance for server config that persists across logouts
const serverConfigStorage = new MMKV({ id: 'server-config' });

const SERVER_KEY = 'custom-server-url';
const LEGACY_HOSTS = new Set(['localhost', '127.0.0.1', '8.141.30.53']);
const LEGACY_PORTS = new Set(['3005']);

function normalizeUrl(url: string): string {
    return url.trim().replace(/\/+$/, '');
}

const DEFAULT_SERVER_URL = normalizeUrl(getDefaultServerUrl());

function shouldResetLegacyCustomUrl(url: string): boolean {
    try {
        const parsed = new URL(url);
        const host = parsed.hostname.toLowerCase();
        const port = parsed.port;
        if (LEGACY_HOSTS.has(host)) return true;
        if (LEGACY_PORTS.has(port)) return true;
        return false;
    } catch {
        // Invalid custom URL should be treated as stale and reset.
        return true;
    }
}

export function getServerUrl(): string {
    const customUrl = serverConfigStorage.getString(SERVER_KEY);
    if (customUrl) {
        const normalizedCustom = normalizeUrl(customUrl);
        if (shouldResetLegacyCustomUrl(normalizedCustom)) {
            // Auto-migrate stale release overrides (localhost / old IP / :3005).
            serverConfigStorage.delete(SERVER_KEY);
        } else {
            return normalizedCustom;
        }
    }

    return DEFAULT_SERVER_URL;
}

export function isHostedCloudConfigured(): boolean {
    return DEFAULT_SERVER_URL.length > 0;
}

export function getHostedConsoleUrl(): string {
    return `${getServerUrl()}/console`;
}

export function isServerAvailable(url: string | null | undefined = getServerUrl()): boolean {
    if (!url || !url.trim()) {
        return false;
    }

    const normalized = normalizeUrl(url);
    if (!normalized) {
        return false;
    }

    try {
        new URL(normalized);
        return true;
    } catch {
        return false;
    }
}

export function requireServerUrl(url: string | null | undefined = getServerUrl()): string {
    if (!isServerAvailable(url)) {
        throw new Error('Server unavailable');
    }

    return normalizeUrl(url!);
}

export function setServerUrl(url: string | null): void {
    if (url && url.trim()) {
        serverConfigStorage.set(SERVER_KEY, normalizeUrl(url));
    } else {
        serverConfigStorage.delete(SERVER_KEY);
    }
}

export function isUsingCustomServer(): boolean {
    const customUrl = serverConfigStorage.getString(SERVER_KEY);
    if (!customUrl) return false;
    const normalized = normalizeUrl(customUrl);
    if (shouldResetLegacyCustomUrl(normalized)) return false;
    return normalized !== DEFAULT_SERVER_URL;
}

export function getServerInfo(): { hostname: string; port?: number; isCustom: boolean } {
    const url = getServerUrl();
    const isCustom = isUsingCustomServer();

    if (!url) {
        return {
            hostname: '',
            port: undefined,
            isCustom
        };
    }
    
    try {
        const parsed = new URL(url);
        const port = parsed.port ? parseInt(parsed.port) : undefined;
        return {
            hostname: parsed.hostname,
            port,
            isCustom
        };
    } catch {
        // Fallback if URL parsing fails
        return {
            hostname: url,
            port: undefined,
            isCustom
        };
    }
}

export function validateServerUrl(url: string): { valid: boolean; error?: string } {
    if (!url || !url.trim()) {
        return { valid: false, error: 'Server URL cannot be empty' };
    }
    
    try {
        const parsed = new URL(url);
        if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
            return { valid: false, error: 'Server URL must use HTTP or HTTPS protocol' };
        }
        return { valid: true };
    } catch {
        return { valid: false, error: 'Invalid URL format' };
    }
}
