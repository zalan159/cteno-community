import { beforeEach, describe, expect, it, vi } from 'vitest';
import AsyncStorage from '@react-native-async-storage/async-storage';
import { evictIfNeeded, loadCache, saveCache, type SessionMessagesCacheEntry } from './messageCache';

const storage = new Map<string, string>();

vi.mock('@react-native-async-storage/async-storage', () => ({
    default: {
        getItem: vi.fn(async (key: string) => storage.get(key) ?? null),
        setItem: vi.fn(async (key: string, value: string) => {
            storage.set(key, value);
        }),
        removeItem: vi.fn(async (key: string) => {
            storage.delete(key);
        }),
        getAllKeys: vi.fn(async () => Array.from(storage.keys())),
        multiGet: vi.fn(async (keys: string[]) => keys.map((key) => [key, storage.get(key) ?? null])),
        multiRemove: vi.fn(async (keys: string[]) => {
            keys.forEach((key) => storage.delete(key));
        }),
    },
}));

function entryWithText(text: string, lastAccessedAt: number): SessionMessagesCacheEntry {
    return {
        messages: [{
            id: `msg-${lastAccessedAt}`,
            localId: null,
            createdAt: lastAccessedAt,
            role: 'assistant',
            text,
        }],
        meta: {
            lastAccessedAt,
            maxSeq: 1,
            hasOlderMessages: false,
        },
    };
}

describe('messageCache', () => {
    beforeEach(() => {
        storage.clear();
        vi.restoreAllMocks();
    });

    it('merges cached pages by message id and keeps the latest pagination metadata', async () => {
        await saveCache('session-1', [
            {
                id: 'm1',
                localId: null,
                seq: 1,
                createdAt: 10,
                role: 'user',
                text: 'first',
            },
            {
                id: 'm2',
                localId: null,
                seq: 2,
                createdAt: 20,
                role: 'assistant',
                text: 'old second',
            },
        ], {
            maxSeq: 50,
            hasOlderMessages: true,
        });

        await saveCache('session-1', [
            {
                id: 'm2',
                localId: null,
                seq: 2,
                createdAt: 20,
                role: 'assistant',
                text: 'new second',
            },
            {
                id: 'm3',
                localId: null,
                seq: 3,
                createdAt: 30,
                role: 'assistant',
                text: 'third',
            },
        ], {
            maxSeq: 100,
            hasOlderMessages: false,
        });

        const cached = await loadCache('session-1');

        expect(cached).not.toBeNull();
        expect(cached?.messages.map((message) => [message.id, message.text])).toEqual([
            ['m1', 'first'],
            ['m2', 'new second'],
            ['m3', 'third'],
        ]);
        expect(cached?.meta.maxSeq).toBe(100);
        expect(cached?.meta.hasOlderMessages).toBe(false);
    });

    it('touches lastAccessedAt when loading cache', async () => {
        vi.spyOn(Date, 'now').mockReturnValue(200);
        await AsyncStorage.setItem('session:session-2:messages', JSON.stringify({
            messages: [{
                id: 'm1',
                localId: null,
                createdAt: 10,
                role: 'user',
                text: 'hello',
            }],
            meta: {
                lastAccessedAt: 100,
                maxSeq: 1,
            },
        }));

        const cached = await loadCache('session-2');
        const persisted = JSON.parse((await AsyncStorage.getItem('session:session-2:messages')) ?? 'null');

        expect(cached?.meta.lastAccessedAt).toBe(200);
        expect(persisted.meta.lastAccessedAt).toBe(200);
    });

    it('evicts the oldest session entries until cache size falls below the target threshold', async () => {
        const oldest = entryWithText('a'.repeat(70), 10);
        const middle = entryWithText('b'.repeat(70), 20);
        const newest = entryWithText('c'.repeat(70), 30);

        const oldestRaw = JSON.stringify(oldest);
        const middleRaw = JSON.stringify(middle);
        const newestRaw = JSON.stringify(newest);

        await AsyncStorage.setItem('session:oldest:messages', oldestRaw);
        await AsyncStorage.setItem('session:middle:messages', middleRaw);
        await AsyncStorage.setItem('session:newest:messages', newestRaw);

        const size = (raw: string) => new TextEncoder().encode(raw).length;
        const totalBytes = size(oldestRaw) + size(middleRaw) + size(newestRaw);
        const targetBytes = size(middleRaw) + size(newestRaw) + 1;

        await evictIfNeeded({
            maxBytes: totalBytes - 1,
            targetBytes,
        });

        expect(await AsyncStorage.getItem('session:oldest:messages')).toBeNull();
        expect(await AsyncStorage.getItem('session:middle:messages')).not.toBeNull();
        expect(await AsyncStorage.getItem('session:newest:messages')).not.toBeNull();
    });
});
