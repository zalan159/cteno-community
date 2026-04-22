import { describe, expect, it } from 'vitest';
import { getHostToolMetadata, getHostToolSubtitle, isHostOwnedTool } from './hostTool';

describe('hostTool helpers', () => {
    it('recognizes injected host-owned tool metadata', () => {
        const tool = {
            input: {
                task: 'Inspect latest run',
                __cteno_host: {
                    owned: true,
                    requestId: 'req-123',
                    source: 'injected_tool',
                },
            },
        };

        expect(isHostOwnedTool(tool)).toBe(true);
        expect(getHostToolMetadata(tool)).toEqual({
            owned: true,
            requestId: 'req-123',
            source: 'injected_tool',
        });
    });

    it('ignores malformed or non-owned metadata', () => {
        expect(isHostOwnedTool({ input: null })).toBe(false);
        expect(isHostOwnedTool({ input: { __cteno_host: { owned: false } } })).toBe(false);
        expect(isHostOwnedTool({ input: { __cteno_host: 'bad-shape' } })).toBe(false);
    });

    it('only injects host subtitle when an existing subtitle is absent', () => {
        const hostTool = {
            input: {
                __cteno_host: {
                    owned: true,
                    requestId: 'req-456',
                    source: 'injected_tool',
                },
            },
        };

        expect(getHostToolSubtitle(hostTool, null)).toBe('Triggered by host runtime');
        expect(getHostToolSubtitle(hostTool, 'Existing detail')).toBe('Existing detail');
        expect(getHostToolSubtitle({ input: {} }, null)).toBeNull();
    });
});
