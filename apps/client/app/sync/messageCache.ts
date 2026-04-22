import AsyncStorage from '@react-native-async-storage/async-storage';

const SESSION_MESSAGES_PREFIX = 'session:';
const SESSION_MESSAGES_SUFFIX = ':messages';
const DEFAULT_MAX_CACHE_BYTES = 100 * 1024 * 1024;
const DEFAULT_TARGET_CACHE_BYTES = 80 * 1024 * 1024;

export type CachedSessionMessage = {
    id: string;
    localId: string | null;
    seq?: number | null;
    createdAt: number;
    role: 'user' | 'assistant';
    text: string;
};

export type SessionMessagesCacheMeta = {
    lastAccessedAt: number;
    maxSeq: number;
    hasOlderMessages?: boolean;
};

export type SessionMessagesCacheEntry = {
    messages: CachedSessionMessage[];
    meta: SessionMessagesCacheMeta;
};

type EvictionOptions = {
    maxBytes?: number;
    targetBytes?: number;
};

type SaveCacheOptions = {
    maxSeq?: number;
    hasOlderMessages?: boolean;
};

function cacheKey(sessionId: string): string {
    return `${SESSION_MESSAGES_PREFIX}${sessionId}${SESSION_MESSAGES_SUFFIX}`;
}

function cacheSizeBytes(value: string): number {
    return new TextEncoder().encode(value).length;
}

function normalizeCachedMessage(input: unknown): CachedSessionMessage | null {
    if (!input || typeof input !== 'object') {
        return null;
    }

    const message = input as Record<string, unknown>;
    if (
        typeof message.id !== 'string' ||
        typeof message.createdAt !== 'number' ||
        (message.role !== 'user' && message.role !== 'assistant') ||
        typeof message.text !== 'string'
    ) {
        return null;
    }

    return {
        id: message.id,
        localId: typeof message.localId === 'string' ? message.localId : null,
        seq: typeof message.seq === 'number' ? message.seq : null,
        createdAt: message.createdAt,
        role: message.role,
        text: message.text,
    };
}

function computeMaxSeq(messages: CachedSessionMessage[], fallback = 0): number {
    const knownSeq = messages.reduce<number>((max, message) => {
        return typeof message.seq === 'number' && Number.isFinite(message.seq)
            ? Math.max(max, message.seq)
            : max;
    }, fallback);

    if (knownSeq > 0) {
        return knownSeq;
    }

    return Math.max(fallback, messages.length);
}

function normalizeCacheEntry(input: unknown): SessionMessagesCacheEntry | null {
    if (!input || typeof input !== 'object') {
        return null;
    }

    const parsed = input as Record<string, unknown>;
    if (!Array.isArray(parsed.messages) || !parsed.meta || typeof parsed.meta !== 'object') {
        return null;
    }

    const messages = parsed.messages
        .map((message) => normalizeCachedMessage(message))
        .filter((message): message is CachedSessionMessage => message !== null)
        .sort((a, b) => a.createdAt - b.createdAt);

    const metaInput = parsed.meta as Record<string, unknown>;
    const lastAccessedAt = typeof metaInput.lastAccessedAt === 'number' ? metaInput.lastAccessedAt : 0;
    const maxSeq = typeof metaInput.maxSeq === 'number' ? metaInput.maxSeq : computeMaxSeq(messages);

    return {
        messages,
        meta: {
            lastAccessedAt,
            maxSeq,
            hasOlderMessages: typeof metaInput.hasOlderMessages === 'boolean' ? metaInput.hasOlderMessages : undefined,
        },
    };
}

function mergeCachedMessages(
    existing: CachedSessionMessage[],
    incoming: CachedSessionMessage[],
): CachedSessionMessage[] {
    const merged = new Map<string, CachedSessionMessage>();

    existing.forEach((message) => {
        merged.set(message.id, message);
    });

    incoming.forEach((message) => {
        const previous = merged.get(message.id);
        if (!previous) {
            merged.set(message.id, message);
            return;
        }

        merged.set(message.id, {
            ...previous,
            ...message,
            localId: message.localId ?? previous.localId,
            seq: message.seq ?? previous.seq ?? null,
        });
    });

    return Array.from(merged.values()).sort((a, b) => a.createdAt - b.createdAt);
}

async function readEntry(sessionId: string): Promise<{ key: string; entry: SessionMessagesCacheEntry } | null> {
    const key = cacheKey(sessionId);
    const raw = await AsyncStorage.getItem(key);
    if (!raw) {
        return null;
    }

    try {
        const entry = normalizeCacheEntry(JSON.parse(raw));
        if (!entry) {
            await AsyncStorage.removeItem(key);
            return null;
        }
        return { key, entry };
    } catch {
        await AsyncStorage.removeItem(key);
        return null;
    }
}

export async function loadCache(sessionId: string): Promise<SessionMessagesCacheEntry | null> {
    const cached = await readEntry(sessionId);
    if (!cached) {
        return null;
    }

    const touchedEntry: SessionMessagesCacheEntry = {
        messages: cached.entry.messages,
        meta: {
            ...cached.entry.meta,
            lastAccessedAt: Date.now(),
            maxSeq: computeMaxSeq(cached.entry.messages, cached.entry.meta.maxSeq),
        },
    };

    await AsyncStorage.setItem(cached.key, JSON.stringify(touchedEntry));
    return touchedEntry;
}

export async function saveCache(
    sessionId: string,
    messages: CachedSessionMessage[],
    options?: SaveCacheOptions,
): Promise<SessionMessagesCacheEntry> {
    const existing = await readEntry(sessionId);
    const mergedMessages = mergeCachedMessages(existing?.entry.messages ?? [], messages);
    const entry: SessionMessagesCacheEntry = {
        messages: mergedMessages,
        meta: {
            lastAccessedAt: Date.now(),
            maxSeq: Math.max(
                options?.maxSeq ?? 0,
                existing?.entry.meta.maxSeq ?? 0,
                computeMaxSeq(mergedMessages),
            ),
            hasOlderMessages: options?.hasOlderMessages ?? existing?.entry.meta.hasOlderMessages,
        },
    };

    await AsyncStorage.setItem(cacheKey(sessionId), JSON.stringify(entry));
    await evictIfNeeded();
    return entry;
}

export async function evictIfNeeded(options?: EvictionOptions): Promise<void> {
    const maxBytes = options?.maxBytes ?? DEFAULT_MAX_CACHE_BYTES;
    const targetBytes = options?.targetBytes ?? DEFAULT_TARGET_CACHE_BYTES;
    const keys = (await AsyncStorage.getAllKeys()).filter(
        (key) => key.startsWith(SESSION_MESSAGES_PREFIX) && key.endsWith(SESSION_MESSAGES_SUFFIX),
    );

    if (keys.length === 0) {
        return;
    }

    const entries = await AsyncStorage.multiGet(keys);
    const invalidKeys: string[] = [];
    const parsedEntries = entries.flatMap(([key, rawValue]) => {
        if (!rawValue) {
            return [];
        }

        try {
            const entry = normalizeCacheEntry(JSON.parse(rawValue));
            if (!entry) {
                invalidKeys.push(key);
                return [];
            }

            return [{
                key,
                sizeBytes: cacheSizeBytes(rawValue),
                lastAccessedAt: entry.meta.lastAccessedAt,
            }];
        } catch {
            invalidKeys.push(key);
            return [];
        }
    });

    if (invalidKeys.length > 0) {
        await AsyncStorage.multiRemove(invalidKeys);
    }

    let totalBytes = parsedEntries.reduce((sum, entry) => sum + entry.sizeBytes, 0);
    if (totalBytes <= maxBytes) {
        return;
    }

    const keysToRemove: string[] = [];
    for (const entry of parsedEntries.sort((a, b) => a.lastAccessedAt - b.lastAccessedAt)) {
        keysToRemove.push(entry.key);
        totalBytes -= entry.sizeBytes;
        if (totalBytes < targetBytes) {
            break;
        }
    }

    if (keysToRemove.length > 0) {
        await AsyncStorage.multiRemove(keysToRemove);
    }
}
