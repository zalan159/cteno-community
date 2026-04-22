import { describe, expect, it } from 'vitest';

import { TYPEWRITER_FRAME_MS, computeTypewriterChunkSize } from './typewriter';

describe('computeTypewriterChunkSize', () => {
    it('keeps the typewriter effect for small live deltas', () => {
        expect(
            computeTypewriterChunkSize({
                pendingChars: 4,
                elapsedMs: TYPEWRITER_FRAME_MS,
            }),
        ).toBe(4);
    });

    it('accelerates when backlog grows so the preview can catch up', () => {
        expect(
            computeTypewriterChunkSize({
                pendingChars: 320,
                elapsedMs: TYPEWRITER_FRAME_MS,
            }),
        ).toBeGreaterThan(40);
    });

    it('never overshoots the available pending characters', () => {
        expect(
            computeTypewriterChunkSize({
                pendingChars: 1,
                elapsedMs: 500,
            }),
        ).toBe(1);
    });
});
