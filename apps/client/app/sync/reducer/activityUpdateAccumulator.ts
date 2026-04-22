import type { ApiEphemeralActivityUpdate } from '../apiTypes';

export class ActivityUpdateAccumulator {
    private pendingUpdates = new Map<string, ApiEphemeralActivityUpdate>();
    private lastEmittedStates = new Map<string, { active: boolean; thinking: boolean; activeAt: number; thinkingStatus?: string }>();
    private timeoutId: ReturnType<typeof setTimeout> | null = null;
    // Per-session debounce timers for thinking true→false transitions
    private thinkingOffTimers = new Map<string, ReturnType<typeof setTimeout>>();
    private static readonly THINKING_OFF_DELAY = 3000; // 3 seconds

    constructor(
        private flushHandler: (updates: Map<string, ApiEphemeralActivityUpdate>) => void,
        private debounceDelay: number = 500
    ) {}

    addUpdate(update: ApiEphemeralActivityUpdate): void {
        const sessionId = update.id;
        const lastState = this.lastEmittedStates.get(sessionId);

        // Check if this is a critical timestamp update (more than half of disconnect timeout old)
        const timeSinceLastUpdate = lastState ? update.activeAt - lastState.activeAt : 0;
        const isCriticalTimestamp = timeSinceLastUpdate > 60000; // Half of 120 second timeout

        // Detect thinking true→false transition: debounce it to avoid flicker
        const isThinkingOff = lastState?.thinking === true && update.thinking === false;
        const isThinkingOn = lastState?.thinking !== true && update.thinking === true;

        // If thinking just turned back on, cancel any pending thinking-off timer
        if (isThinkingOn) {
            const offTimer = this.thinkingOffTimers.get(sessionId);
            if (offTimer) {
                clearTimeout(offTimer);
                this.thinkingOffTimers.delete(sessionId);
            }
        }

        // If thinking is going from true→false, defer the update
        if (isThinkingOff) {
            // Store the update but don't flush yet
            this.pendingUpdates.set(sessionId, update);

            // If a timer is already running, just update the pending data — don't reset the timer.
            // Keepalives arrive every 2s; resetting a 3s timer each time means it never fires.
            if (this.thinkingOffTimers.has(sessionId)) {
                return;
            }

            this.thinkingOffTimers.set(sessionId, setTimeout(() => {
                this.thinkingOffTimers.delete(sessionId);
                // Only flush if the pending update for this session still has thinking=false
                // (it might have been overwritten by a thinking=true update in the meantime)
                const current = this.pendingUpdates.get(sessionId);
                if (current && !current.thinking) {
                    this.flushPendingUpdates();
                }
            }, ActivityUpdateAccumulator.THINKING_OFF_DELAY));
            return;
        }

        // Check if this is a significant state change that needs immediate emission.
        // Context/compression metrics no longer flow through heartbeat updates.
        const isSignificantChange = !lastState ||
            lastState.active !== update.active ||
            lastState.thinking !== update.thinking ||
            lastState.thinkingStatus !== update.thinkingStatus ||
            isCriticalTimestamp;

        if (isSignificantChange) {
            // Cancel any pending timeout
            if (this.timeoutId) {
                clearTimeout(this.timeoutId);
                this.timeoutId = null;
            }

            // Add the immediate update to pending updates
            this.pendingUpdates.set(sessionId, update);

            // Flush all pending updates together (batched)
            this.flushPendingUpdates();
        } else {
            // Accumulate for debounced emission (only timestamp updates)
            this.pendingUpdates.set(sessionId, update);

            // Only start a new timer if one isn't already running
            if (!this.timeoutId) {
                this.timeoutId = setTimeout(() => {
                    this.flushPendingUpdates();
                    this.timeoutId = null;
                }, this.debounceDelay);
            }
            // Don't reset the timer for subsequent updates - let it fire!
        }
    }

    private flushPendingUpdates(): void {
        if (this.pendingUpdates.size > 0) {
            // Create a copy of the pending updates
            const updatesToFlush = new Map(this.pendingUpdates);

            // Emit all updates in a single batch
            this.flushHandler(updatesToFlush);

            // Update last emitted states for all flushed updates
            for (const [sessionId, update] of updatesToFlush) {
                this.lastEmittedStates.set(sessionId, {
                    active: update.active,
                    thinking: update.thinking,
                    activeAt: update.activeAt,
                    thinkingStatus: update.thinkingStatus,
                });
            }

            // Clear pending updates
            this.pendingUpdates.clear();
        }
    }

    cancel(): void {
        if (this.timeoutId) {
            clearTimeout(this.timeoutId);
            this.timeoutId = null;
        }
        for (const timer of this.thinkingOffTimers.values()) {
            clearTimeout(timer);
        }
        this.thinkingOffTimers.clear();
        this.pendingUpdates.clear();
    }

    reset(): void {
        this.cancel();
        this.lastEmittedStates.clear();
    }

    flush(): void {
        if (this.timeoutId) {
            clearTimeout(this.timeoutId);
            this.timeoutId = null;
        }
        for (const timer of this.thinkingOffTimers.values()) {
            clearTimeout(timer);
        }
        this.thinkingOffTimers.clear();
        this.flushPendingUpdates();
    }
}
