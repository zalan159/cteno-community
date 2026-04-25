import { useMemo } from 'react';
import { storage } from '@/sync/storage';
import type { VendorQuota, VendorQuotaId } from '@/sync/storageTypes';

/**
 * Resolves machine-level plan quota for a given vendor. The store keys entries
 * as `${machineId}:${vendor}`; callers pass both values explicitly because a
 * given desktop may be connected to multiple machines (local + remote).
 */
export function useVendorQuota(
    machineId: string | null | undefined,
    vendor: VendorQuotaId | null | undefined,
): VendorQuota | null {
    const entries = storage((s) => s.vendorQuota);
    return useMemo(() => {
        if (!machineId || !vendor) return null;
        return entries[`${machineId}:${vendor}`] ?? null;
    }, [entries, machineId, vendor]);
}

/**
 * Find the "most restrictive" window or bucket for the small indicator.
 *
 * - For windows-shape vendors (Claude / Codex): return `fiveHour` if
 *   present, else `weekly`, else any window. This matches the
 *   "default-show 5h, click to see 7d" UX we want.
 * - For buckets-shape vendors (Gemini): prefer the bucket matching
 *   `preferredModelId` (current session's model) if we can name it, else
 *   the bucket with the highest `usedPercent` — i.e. the one closest to
 *   exhaustion, which is what the user cares about at a glance.
 */
export function pickPrimaryQuota(
    quota: VendorQuota | null,
    preferredModelId?: string | null,
): {
    usedPercent: number;
    resetsAt?: number;
    label: string;
} | null {
    if (!quota) return null;
    if (quota.error) return null;

    if (quota.shape === 'windows') {
        const windows = quota.windows ?? {};
        const keyOrder = ['fiveHour', 'weekly', 'weeklyOpus', 'weeklySonnet'];
        for (const key of keyOrder) {
            const w = windows[key];
            if (!w) continue;
            return {
                usedPercent: w.usedPercent,
                resetsAt: w.resetsAt,
                label: labelForWindowKey(key),
            };
        }
        const first = Object.entries(windows)[0];
        if (!first) return null;
        return {
            usedPercent: first[1].usedPercent,
            resetsAt: first[1].resetsAt,
            label: first[0],
        };
    }

    const buckets = quota.buckets ?? [];
    if (buckets.length === 0) return null;
    if (preferredModelId) {
        const hit = buckets.find((b) => b.modelId === preferredModelId);
        if (hit) {
            return {
                usedPercent: hit.usedPercent,
                resetsAt: hit.resetsAt,
                label: hit.modelId,
            };
        }
    }
    const worst = [...buckets].sort((a, b) => b.usedPercent - a.usedPercent)[0];
    return {
        usedPercent: worst.usedPercent,
        resetsAt: worst.resetsAt,
        label: worst.modelId,
    };
}

export function labelForWindowKey(key: string): string {
    switch (key) {
        case 'fiveHour': return '5h';
        case 'weekly': return '7d';
        case 'weeklyOpus': return 'Opus 7d';
        case 'weeklySonnet': return 'Sonnet 7d';
        case 'overage': return 'overage';
        default:
            if (key.startsWith('window_')) return key.slice('window_'.length);
            return key;
    }
}

export function formatRemainingPercent(usedPercent: number): string {
    const remaining = Math.max(0, Math.min(100, 100 - usedPercent));
    return `${Math.round(remaining)}%`;
}

export function formatResetCountdown(resetsAt?: number): string {
    if (!resetsAt) return '';
    const delta = resetsAt * 1000 - Date.now();
    if (delta <= 0) return '已重置';
    const minutes = Math.floor(delta / 60000);
    if (minutes < 60) return `${minutes}min 后重置`;
    const hours = Math.floor(minutes / 60);
    const mins = minutes - hours * 60;
    if (hours < 24) {
        return mins > 0 ? `${hours}h ${mins}min 后重置` : `${hours}h 后重置`;
    }
    const days = Math.floor(hours / 24);
    const hoursRem = hours - days * 24;
    return hoursRem > 0 ? `${days}d ${hoursRem}h 后重置` : `${days}d 后重置`;
}

export function remainingColor(usedPercent: number): string {
    // Semantic flip: we care about remaining, so lower remaining → warmer color.
    const remaining = 100 - usedPercent;
    if (remaining <= 15) return '#FF3B30';
    if (remaining <= 30) return '#FF9500';
    return '#8E8E93';
}
