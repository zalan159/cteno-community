import * as React from 'react';
import { Session } from '@/sync/storageTypes';
import { t } from '@/text';
import { getVendorAvatarId } from './vendorIcons';
import { useUnistyles } from 'react-native-unistyles';

export type SessionState = 'disconnected' | 'thinking' | 'compressing' | 'waiting' | 'permission_required';

export interface CompressionInfo {
    contextTokens: number;
    compressionThreshold: number;
    percentage: number;
    color: string;
    text: string;
}

export interface SessionStatus {
    state: SessionState;
    isConnected: boolean;
    statusText: string;
    shouldShowStatus: boolean;
    statusColor: string;
    statusDotColor: string;
    isPulsing?: boolean;
    compressionInfo?: CompressionInfo;
}

function inferCompressionVendor(session: Session): 'cteno' | 'claude' | 'codex' | 'gemini' {
    const metadataVendor = session.metadata?.vendor?.trim().toLowerCase();
    if (metadataVendor === 'cteno' || metadataVendor === 'claude' || metadataVendor === 'codex' || metadataVendor === 'gemini') {
        return metadataVendor;
    }

    const flavor = session.metadata?.flavor?.trim().toLowerCase() ?? '';
    if (flavor.includes('codex') || flavor.includes('openai') || flavor.includes('gpt')) return 'codex';
    if (flavor.includes('claude')) return 'claude';
    if (flavor.includes('gemini')) return 'gemini';
    return 'cteno';
}

function defaultCompressionThresholdForSession(session: Session): number {
    switch (inferCompressionVendor(session)) {
        case 'claude':
        case 'gemini':
            return 1_000_000;
        case 'codex':
        case 'cteno':
        default:
            return 256_000;
    }
}

/**
 * Get the current state of a session based on presence and thinking status.
 * Uses centralized session state from storage.ts
 */
export function useSessionStatus(session: Session): SessionStatus {
    const { theme } = useUnistyles();
    const isOnline = session.presence === "online";
    const hasPermissions = (session.agentState?.requests && Object.keys(session.agentState.requests).length > 0 ? true : false);
    const requiresAction = session.thinkingStatus === 'requires_action';

    // Use session.id as a stable seed so the message doesn't change on every thinking toggle.
    // Only pick a new message when the session identity changes.
    const vibingMessage = React.useMemo(() => {
        let hash = 0;
        const id = session.id || '';
        for (let i = 0; i < id.length; i++) {
            hash = ((hash << 5) - hash + id.charCodeAt(i)) | 0;
        }
        return vibingMessages[Math.abs(hash) % vibingMessages.length].toLowerCase() + '…';
    }, [session.id]);

    const compressionInfo = React.useMemo((): CompressionInfo | undefined => {
        const contextTokens = session.contextTokens ?? session.latestUsage?.contextSize ?? 0;
        const compressionThreshold = defaultCompressionThresholdForSession(session);
        if (!Number.isFinite(contextTokens) || !Number.isFinite(compressionThreshold) || compressionThreshold <= 0) {
            return undefined;
        }
        const pct = (contextTokens / compressionThreshold) * 100;
        const formatK = (n: number) => {
            if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
            return n >= 1000 ? `${Math.round(n / 1000)}K` : `${n}`;
        };
        const color = pct >= 45 ? '#FF3B30' : pct >= 35 ? '#FF9500' : theme.colors.status.default;
        return {
            contextTokens,
            compressionThreshold,
            percentage: Math.round(pct),
            color,
            text: `${formatK(contextTokens)}/${formatK(compressionThreshold)}`
        };
    }, [session.contextTokens, session.latestUsage?.contextSize, session.metadata?.vendor, session.metadata?.flavor, theme.colors.status.default]);

    if (!isOnline) {
        return {
            state: 'disconnected',
            isConnected: false,
            statusText: t('status.lastSeen', { time: formatLastSeen(session.activeAt, false) }),
            shouldShowStatus: true,
            statusColor: theme.colors.status.disconnected,
            statusDotColor: theme.colors.status.disconnected,
        };
    }

    // Check if permission is required
    if (hasPermissions || requiresAction) {
        return {
            state: 'permission_required',
            isConnected: true,
            statusText: t('status.permissionRequired'),
            shouldShowStatus: true,
            statusColor: '#FF9500',
            statusDotColor: '#FF9500',
            isPulsing: true,
            compressionInfo,
        };
    }

    if (session.thinking === true) {
        const isCompressing = session.thinkingStatus === 'compressing';
        return {
            state: isCompressing ? 'compressing' : 'thinking',
            isConnected: true,
            statusText: isCompressing ? 'compressing conversation\u2026' : vibingMessage,
            shouldShowStatus: true,
            statusColor: isCompressing ? theme.colors.status.default : '#007AFF',
            statusDotColor: isCompressing ? theme.colors.status.default : '#007AFF',
            isPulsing: true,
            compressionInfo,
        };
    }

    return {
        state: 'waiting',
        isConnected: true,
        statusText: t('status.online'),
        shouldShowStatus: false,
        statusColor: '#34C759',
        statusDotColor: '#34C759',
        compressionInfo,
    };
}

/**
 * Extracts a display name from a session's metadata path.
 * Returns the last segment of the path, or 'unknown' if no path is available.
 */
export function getSessionName(session: Session): string {
    if (session.metadata?.summary) {
        return session.metadata.summary.text;
    } else if (session.metadata) {
        const segments = session.metadata.path.split('/').filter(Boolean);
        const lastSegment = segments.pop();
        if (!lastSegment) {
            return t('status.unknown');
        }
        return lastSegment;
    }
    return t('status.unknown');
}

/**
 * Generates a deterministic avatar ID from machine ID and path.
 * This ensures the same machine + path combination always gets the same avatar.
 */
export function getSessionAvatarId(session: Session): string {
    const vendor = session.metadata?.vendor?.trim().toLowerCase()
        || session.metadata?.flavor?.trim().toLowerCase()
        || null;
    if (vendor === 'cteno' || vendor === 'claude' || vendor === 'codex' || vendor === 'gemini') {
        return getVendorAvatarId(vendor);
    }
    if (session.metadata?.machineId && session.metadata?.path) {
        // Combine machine ID and path for a unique, deterministic avatar
        return `${session.metadata.machineId}:${session.metadata.path}`;
    }
    // Fallback to session ID if metadata is missing
    return session.id;
}

/**
 * Formats a path relative to home directory if possible.
 * If the path starts with the home directory, replaces it with ~
 * Otherwise returns the full path.
 */
export function formatPathRelativeToHome(path: string, homeDir?: string): string {
    if (!homeDir) return path;
    
    // Normalize paths to handle trailing slashes
    const normalizedHome = homeDir.endsWith('/') ? homeDir.slice(0, -1) : homeDir;
    const normalizedPath = path;
    
    // Check if path starts with home directory
    if (normalizedPath.startsWith(normalizedHome)) {
        // Replace home directory with ~
        const relativePath = normalizedPath.slice(normalizedHome.length);
        // Add ~ and ensure there's a / after it if needed
        if (relativePath.startsWith('/')) {
            return '~' + relativePath;
        } else if (relativePath === '') {
            return '~';
        } else {
            return '~/' + relativePath;
        }
    }
    
    return path;
}

/**
 * Returns the session path for the subtitle, with proxy model tag if applicable.
 */
export function getSessionSubtitle(session: Session): string {
    if (session.metadata) {
        let subtitle = formatPathRelativeToHome(session.metadata.path, session.metadata.homeDir);
        if (session.metadata.proxyModelId) {
            subtitle += ` · ${session.metadata.proxyModelId}`;
        }
        return subtitle;
    }
    return t('status.unknown');
}

/**
 * Checks if a session is currently online based on the active flag.
 * A session is considered online if the active flag is true.
 */
export function isSessionOnline(session: Session): boolean {
    return session.active;
}

/**
 * True when the session's messages are stored locally on a machine we own, so
 * the frontend should read them via local IPC (`machineGetSessionMessages`)
 * rather than the server's `/v1/sessions/:id/messages` endpoint.
 *
 * Two signals qualify a session as local-first:
 *   - Legacy explicit marker: `metadata.host === 'local-shell'` — emitted by
 *     the desktop daemon's own enumerator (see `desktop/src/host/sessions.rs`).
 *   - Vendor flavor: `cteno / claude / codex / gemini / persona` — these are
 *     all run locally by the desktop daemon's agent executors, even when the
 *     server-relayed `new-session` event overwrites `host` with the hostname.
 *
 * We deliberately accept either signal because the server relay and the
 * local enumerator race on session metadata — whichever writes last wins,
 * and we don't want one ordering to silently break message fetching.
 */
export function isLocalFirstSession(metadata: Session['metadata']): boolean {
    if (!metadata) return false;
    if (metadata.host === 'local-shell') return true;

    const flavor = metadata.flavor?.trim().toLowerCase() ?? '';
    if (!flavor) return false;
    return (
        flavor === 'cteno' ||
        flavor === 'claude' ||
        flavor === 'codex' ||
        flavor === 'gemini' ||
        flavor === 'persona' ||
        flavor === 'task' ||
        flavor === 'workspace-member' ||
        flavor === 'local-agent-session' ||
        flavor.includes('cteno') ||
        flavor.includes('claude') ||
        flavor.includes('codex') ||
        flavor.includes('gemini')
    );
}

/**
 * Checks if a session should be shown in the active sessions group.
 * Uses the active flag directly.
 */
export function isSessionActive(session: Session): boolean {
    return session.active;
}

/**
 * Formats OS platform string into a more readable format
 */
export function formatOSPlatform(platform?: string): string {
    if (!platform) return '';

    const osMap: Record<string, string> = {
        'darwin': 'macOS',
        'win32': 'Windows',
        'linux': 'Linux',
        'android': 'Android',
        'ios': 'iOS',
        'aix': 'AIX',
        'freebsd': 'FreeBSD',
        'openbsd': 'OpenBSD',
        'sunos': 'SunOS'
    };

    return osMap[platform.toLowerCase()] || platform;
}

/**
 * Formats the last seen time of a session into a human-readable relative time.
 * @param activeAt - Timestamp when the session was last active
 * @param isActive - Whether the session is currently active
 * @returns Formatted string like "Active now", "5 minutes ago", "2 hours ago", or a date
 */
export function formatLastSeen(activeAt: number, isActive: boolean = false): string {
    if (isActive) {
        return t('status.activeNow');
    }

    const now = Date.now();
    const diffMs = now - activeAt;
    const diffSeconds = Math.floor(diffMs / 1000);
    const diffMinutes = Math.floor(diffSeconds / 60);
    const diffHours = Math.floor(diffMinutes / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffSeconds < 60) {
        return t('time.justNow');
    } else if (diffMinutes < 60) {
        return t('time.minutesAgo', { count: diffMinutes });
    } else if (diffHours < 24) {
        return t('time.hoursAgo', { count: diffHours });
    } else if (diffDays < 7) {
        return t('sessionHistory.daysAgo', { count: diffDays });
    } else {
        // Format as date
        const date = new Date(activeAt);
        const options: Intl.DateTimeFormatOptions = {
            month: 'short',
            day: 'numeric',
            year: date.getFullYear() !== new Date().getFullYear() ? 'numeric' : undefined
        };
        return date.toLocaleDateString(undefined, options);
    }
}

const vibingMessages = ["Accomplishing", "Actioning", "Actualizing", "Baking", "Booping", "Brewing", "Calculating", "Cerebrating", "Channelling", "Churning", "Clauding", "Coalescing", "Cogitating", "Computing", "Combobulating", "Concocting", "Conjuring", "Considering", "Contemplating", "Cooking", "Crafting", "Creating", "Crunching", "Deciphering", "Deliberating", "Determining", "Discombobulating", "Divining", "Doing", "Effecting", "Elucidating", "Enchanting", "Envisioning", "Finagling", "Flibbertigibbeting", "Forging", "Forming", "Frolicking", "Generating", "Germinating", "Hatching", "Herding", "Honking", "Ideating", "Imagining", "Incubating", "Inferring", "Manifesting", "Marinating", "Meandering", "Moseying", "Mulling", "Mustering", "Musing", "Noodling", "Percolating", "Perusing", "Philosophising", "Pontificating", "Pondering", "Processing", "Puttering", "Puzzling", "Reticulating", "Ruminating", "Scheming", "Schlepping", "Shimmying", "Simmering", "Smooshing", "Spelunking", "Spinning", "Stewing", "Sussing", "Synthesizing", "Thinking", "Tinkering", "Transmuting", "Unfurling", "Unravelling", "Vibing", "Wandering", "Whirring", "Wibbling", "Wizarding", "Working", "Wrangling"];
