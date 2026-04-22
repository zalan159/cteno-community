import { z } from 'zod';

export const LocalProxyUsageRecordSchema = z.object({
    key: z.string(),
    sessionId: z.string(),
    timestamp: z.number(),
    totalTokens: z.number(),
    inputTokens: z.number(),
    outputTokens: z.number(),
    cacheCreationTokens: z.number(),
    cacheReadTokens: z.number(),
    totalCostYuan: z.number(),
    inputCostYuan: z.number(),
    outputCostYuan: z.number(),
});

export type LocalProxyUsageRecord = z.infer<typeof LocalProxyUsageRecordSchema>;

export const LocalProxyUsageSchema = z.object({
    records: z.record(LocalProxyUsageRecordSchema),
});

export type LocalProxyUsage = z.infer<typeof LocalProxyUsageSchema>;

export type LocalProxyUsagePeriod = 'today' | '7days' | '30days';

export interface LocalProxyUsageDay {
    date: string;
    costYuan: number;
    tokens: number;
    requests: number;
}

export interface LocalProxyUsageSummary {
    records: LocalProxyUsageRecord[];
    totalCostYuan: number;
    totalTokens: number;
    totalInputTokens: number;
    totalOutputTokens: number;
    totalCacheCreationTokens: number;
    totalCacheReadTokens: number;
    requestCount: number;
    byDay: LocalProxyUsageDay[];
}

export const localProxyUsageDefaults: LocalProxyUsage = {
    records: {},
};

Object.freeze(localProxyUsageDefaults);

export function localProxyUsageParse(localProxyUsage: unknown): LocalProxyUsage {
    const parsed = LocalProxyUsageSchema.safeParse(localProxyUsage);
    if (!parsed.success) {
        return { ...localProxyUsageDefaults };
    }
    return {
        records: parsed.data.records,
    };
}

function readNumericValue(
    values: Record<string, number>,
    key: string,
): number {
    return typeof values[key] === 'number' ? values[key] : 0;
}

export function createLocalProxyUsageRecord(update: {
    key: string;
    sessionId: string;
    timestamp: number;
    tokens: Record<string, number>;
    cost: Record<string, number>;
}): LocalProxyUsageRecord {
    return {
        key: update.key,
        sessionId: update.sessionId,
        timestamp: update.timestamp,
        totalTokens: readNumericValue(update.tokens, 'total'),
        inputTokens: readNumericValue(update.tokens, 'input'),
        outputTokens: readNumericValue(update.tokens, 'output'),
        cacheCreationTokens: readNumericValue(update.tokens, 'cache_creation'),
        cacheReadTokens: readNumericValue(update.tokens, 'cache_read'),
        totalCostYuan: readNumericValue(update.cost, 'total'),
        inputCostYuan: readNumericValue(update.cost, 'input'),
        outputCostYuan: readNumericValue(update.cost, 'output'),
    };
}

export function upsertLocalProxyUsageRecord(
    state: LocalProxyUsage,
    record: LocalProxyUsageRecord,
): LocalProxyUsage {
    return {
        records: {
            ...state.records,
            [record.key]: record,
        },
    };
}

function getPeriodStart(period: LocalProxyUsagePeriod): number {
    const now = new Date();
    switch (period) {
        case 'today': {
            const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
            return start.getTime();
        }
        case '7days':
            return now.getTime() - 7 * 24 * 60 * 60 * 1000;
        case '30days':
            return now.getTime() - 30 * 24 * 60 * 60 * 1000;
    }
}

function formatLocalDate(timestamp: number): string {
    const date = new Date(timestamp);
    const year = date.getFullYear();
    const month = `${date.getMonth() + 1}`.padStart(2, '0');
    const day = `${date.getDate()}`.padStart(2, '0');
    return `${year}-${month}-${day}`;
}

export function buildLocalProxyUsageSummary(
    state: LocalProxyUsage,
    period: LocalProxyUsagePeriod,
): LocalProxyUsageSummary {
    const periodStart = getPeriodStart(period);
    const records = Object.values(state.records)
        .filter((record) => record.timestamp >= periodStart)
        .sort((a, b) => b.timestamp - a.timestamp);

    const byDay = new Map<string, LocalProxyUsageDay>();

    const summary: LocalProxyUsageSummary = {
        records,
        totalCostYuan: 0,
        totalTokens: 0,
        totalInputTokens: 0,
        totalOutputTokens: 0,
        totalCacheCreationTokens: 0,
        totalCacheReadTokens: 0,
        requestCount: records.length,
        byDay: [],
    };

    for (const record of records) {
        summary.totalCostYuan += record.totalCostYuan;
        summary.totalTokens += record.totalTokens;
        summary.totalInputTokens += record.inputTokens;
        summary.totalOutputTokens += record.outputTokens;
        summary.totalCacheCreationTokens += record.cacheCreationTokens;
        summary.totalCacheReadTokens += record.cacheReadTokens;

        const date = formatLocalDate(record.timestamp);
        const existing = byDay.get(date);
        if (existing) {
            existing.costYuan += record.totalCostYuan;
            existing.tokens += record.totalTokens;
            existing.requests += 1;
        } else {
            byDay.set(date, {
                date,
                costYuan: record.totalCostYuan,
                tokens: record.totalTokens,
                requests: 1,
            });
        }
    }

    summary.byDay = Array.from(byDay.values()).sort((a, b) => a.date.localeCompare(b.date));
    return summary;
}
