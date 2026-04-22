const TYPEWRITER_MIN_CHUNK = 2;
const TYPEWRITER_MAX_CHUNK = 96;
const TYPEWRITER_BASE_CHARS_PER_SECOND = 90;
const TYPEWRITER_TARGET_DRAIN_MS = 220;

export const TYPEWRITER_FRAME_MS = 50;

export function nowForTypewriter(): number {
    const perfNow = globalThis.performance?.now;
    return typeof perfNow === 'function' ? perfNow.call(globalThis.performance) : Date.now();
}

export function computeTypewriterChunkSize(input: {
    pendingChars: number;
    elapsedMs: number;
}): number {
    const pendingChars = Math.max(0, input.pendingChars);
    if (pendingChars === 0) {
        return 0;
    }

    const elapsedMs = Math.max(16, input.elapsedMs);
    const liveRateChunk = Math.ceil((TYPEWRITER_BASE_CHARS_PER_SECOND * elapsedMs) / 1000);
    const backlogCatchupChunk = Math.ceil((pendingChars * elapsedMs) / TYPEWRITER_TARGET_DRAIN_MS);

    return Math.min(
        pendingChars,
        Math.min(
            TYPEWRITER_MAX_CHUNK,
            Math.max(TYPEWRITER_MIN_CHUNK, liveRateChunk, backlogCatchupChunk),
        ),
    );
}
