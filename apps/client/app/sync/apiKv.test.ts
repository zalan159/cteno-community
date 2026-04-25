import { afterEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
    authedFetch: vi.fn(),
    getServerUrl: vi.fn(() => 'https://cteno.example'),
    isServerAvailable: vi.fn(() => true),
}));

vi.mock('./authedFetch', () => ({
    authedFetch: mocks.authedFetch,
    NotAuthenticatedError: class NotAuthenticatedError extends Error { },
}));

vi.mock('./serverConfig', () => ({
    getServerUrl: mocks.getServerUrl,
    isServerAvailable: mocks.isServerAvailable,
}));

async function importApiKv() {
    return await import('./apiKv');
}

afterEach(() => {
    vi.unstubAllEnvs();
    mocks.authedFetch.mockReset();
    mocks.getServerUrl.mockReset();
    mocks.getServerUrl.mockReturnValue('https://cteno.example');
    mocks.isServerAvailable.mockReset();
    mocks.isServerAvailable.mockReturnValue(true);
});

describe('apiKv cloud sync gating', () => {
    it('does not read from server-backed KV when cloud sync is disabled', async () => {
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'false');
        const { kvGet, kvList, kvBulkGet } = await importApiKv();

        await expect(kvGet('todo.index')).resolves.toBeNull();
        await expect(kvList({ prefix: 'todo.' })).resolves.toEqual({ items: [] });
        await expect(kvBulkGet(['todo.index'])).resolves.toEqual({ values: [] });

        expect(mocks.authedFetch).not.toHaveBeenCalled();
    });

    it('treats mutations as local no-ops when cloud sync is disabled', async () => {
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'false');
        const { kvMutate } = await importApiKv();

        await expect(kvMutate([
            { key: 'todo.one', value: 'encrypted', version: -1 },
            { key: 'todo.two', value: null, version: 3 },
        ])).resolves.toEqual({
            success: true,
            results: [
                { key: 'todo.one', version: 0 },
                { key: 'todo.two', version: 4 },
            ],
        });

        expect(mocks.authedFetch).not.toHaveBeenCalled();
    });

    it('uses server-backed KV when cloud sync is enabled', async () => {
        vi.stubEnv('EXPO_PUBLIC_CLOUD_SYNC_ENABLED', 'true');
        mocks.authedFetch.mockResolvedValue(new Response(
            JSON.stringify({ success: true, results: [{ key: 'todo.one', version: 1 }] }),
            { status: 200 },
        ));
        const { kvMutate } = await importApiKv();

        await expect(kvMutate([
            { key: 'todo.one', value: 'encrypted', version: -1 },
        ])).resolves.toEqual({
            success: true,
            results: [{ key: 'todo.one', version: 1 }],
        });

        expect(mocks.authedFetch).toHaveBeenCalledWith(
            'https://cteno.example/v1/kv',
            expect.objectContaining({ method: 'POST' }),
        );
    });
});
