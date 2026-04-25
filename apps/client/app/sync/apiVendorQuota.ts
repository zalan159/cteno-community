/**
 * Machine-level vendor quota (Claude / Codex / Gemini plan/rate limits).
 *
 * Fetched via the `{machineId}:quota-read` RPC exposed by
 * `apps/client/desktop/src/usage_monitor.rs`. The daemon owns one poller per
 * vendor and this call just reads the cached snapshot — frontend polls every
 * 60s to surface fresh data.
 */

import { apiSocket } from './apiSocket';
import { storage } from './storage';
import type { VendorQuota, VendorQuotaId } from './storageTypes';
import { frontendLog } from '@/utils/tauri';

export interface VendorQuotaSnapshot {
    entries: Record<VendorQuotaId, VendorQuota>;
}

type QuotaReadResponse = Record<string, VendorQuota>;

/**
 * Pull the latest snapshot from the daemon and merge it into the global
 * store. Safe to call concurrently — storage's `applyVendorQuota` replaces
 * the machine's block atomically.
 */
export async function fetchVendorQuota(machineId: string): Promise<void> {
    frontendLog(`[vendorQuota] fetch starting for ${machineId}`);
    try {
        const response = await apiSocket.machineRPC<QuotaReadResponse, {}>(
            machineId,
            'quota-read',
            {},
        );

        // Daemon sometimes wraps responses in `{ entries: {...} }` (from
        // `VendorQuotaMap` serde flattening); tolerate either shape.
        const payload: QuotaReadResponse =
            response && typeof response === 'object' && 'entries' in response
                ? ((response as unknown as { entries: QuotaReadResponse }).entries ?? {})
                : response ?? {};

        frontendLog(`[vendorQuota] fetch ok for ${machineId} vendors=${JSON.stringify(Object.keys(payload))} raw=${JSON.stringify(response).slice(0, 400)}`);
        storage.getState().applyVendorQuota(machineId, payload);
    } catch (error) {
        frontendLog(`[vendorQuota] fetch failed for ${machineId}: ${String(error)}`, 'warn');
    }
}
