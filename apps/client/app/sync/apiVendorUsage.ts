/**
 * Machine-level vendor usage (Claude / Codex / Gemini rate limits).
 *
 * Fetched via the `{machineId}:usage-read` RPC exposed by
 * `apps/client/desktop/src/usage_monitor.rs`. The daemon owns one poller per
 * vendor and this call just reads the cached snapshot — frontend polls every
 * 60s to surface fresh data.
 */

import { apiSocket } from './apiSocket';
import { storage } from './storage';
import type { VendorUsage, VendorUsageId } from './storageTypes';
import { frontendLog } from '@/utils/tauri';

export interface VendorUsageSnapshot {
    entries: Record<VendorUsageId, VendorUsage>;
}

type UsageReadResponse = Record<string, VendorUsage>;

/**
 * Pull the latest snapshot from the daemon and merge it into the global
 * store. Safe to call concurrently — storage's `applyVendorUsage` replaces
 * the machine's block atomically.
 */
export async function fetchVendorUsage(machineId: string): Promise<void> {
    frontendLog(`[vendorUsage] fetch starting for ${machineId}`);
    try {
        const response = await apiSocket.machineRPC<UsageReadResponse, {}>(
            machineId,
            'usage-read',
            {},
        );

        // Daemon sometimes wraps responses in `{ entries: {...} }` (from
        // `VendorUsageMap` serde flattening); tolerate either shape.
        const payload: UsageReadResponse =
            response && typeof response === 'object' && 'entries' in response
                ? ((response as unknown as { entries: UsageReadResponse }).entries ?? {})
                : response ?? {};

        frontendLog(`[vendorUsage] fetch ok for ${machineId} vendors=${JSON.stringify(Object.keys(payload))} raw=${JSON.stringify(response).slice(0, 400)}`);
        storage.getState().applyVendorUsage(machineId, payload);
    } catch (error) {
        frontendLog(`[vendorUsage] fetch failed for ${machineId}: ${String(error)}`, 'warn');
    }
}
