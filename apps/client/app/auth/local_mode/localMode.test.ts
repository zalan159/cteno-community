import { afterEach, describe, expect, it, vi } from 'vitest';

const isTauriMock = vi.fn();

vi.mock('@/utils/tauri', () => ({
    isTauri: () => isTauriMock(),
}));

afterEach(() => {
    vi.resetModules();
    isTauriMock.mockReset();
});

describe('localMode', () => {
    it('enables desktop local mode in Tauri', async () => {
        isTauriMock.mockReturnValue(true);
        const { isDesktopLocalModeEnabled } = await import('./localMode');

        expect(isDesktopLocalModeEnabled()).toBe(true);
    });

    it('disables desktop local mode outside Tauri', async () => {
        isTauriMock.mockReturnValue(false);
        const { isDesktopLocalModeEnabled } = await import('./localMode');

        expect(isDesktopLocalModeEnabled()).toBe(false);
    });
});
