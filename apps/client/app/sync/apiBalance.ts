import { backoff } from '@/utils/time';
import { authedFetch } from './authedFetch';
import { requireServerUrl } from './serverConfig';

// ── Types ──────────────────────────────────────────────

export interface BalanceStatus {
    balanceYuan: number;
    totalTopUp: number;
    totalUsed: number;
    rateYuanPerMToken: number;
}

export interface ProxyModelUsage {
    modelId: string;
    modelName: string;
    costYuan: number;
    tokens: number;
}

export interface ProxyDayUsage {
    date: string;
    costYuan: number;
}

export interface ProxyMachineUsage {
    machineId: string;
    costYuan: number;
    tokens: number;
}

export interface PublicProxyModel {
    id: string;
    name: string;
    inputRate: number;
    outputRate: number;
    cacheHitInputRate?: number;
    contextWindowTokens?: number;
    description?: string;
    isCompressModel?: boolean;
    isFree?: boolean;
    supportsVision?: boolean;
    supportsComputerUse?: boolean;
    supportsFunctionCalling?: boolean;
    supportsImageOutput?: boolean;
    apiFormat?: 'anthropic' | 'openai' | 'gemini' | string;
    temperature?: number;
    thinking?: boolean;
}

export interface ProxyUsageSummary {
    totalCostYuan: number;
    totalTokens: number;
    byModel: ProxyModelUsage[];
    byDay: ProxyDayUsage[];
    byMachine: ProxyMachineUsage[];
}

export interface LedgerEntry {
    id: string;
    type: 'usage' | 'topup';
    amountYuan: number;
    balanceAfter: number;
    description: string | null;
    modelId: string | null;
    modelName: string | undefined;
    tokens: number | null;
    machineId: string | null;
    createdAt: number;
}

export interface LedgerResponse {
    entries: LedgerEntry[];
    nextCursor?: string;
}

// ── API calls ──────────────────────────────────────────

export async function fetchBalanceStatus(): Promise<BalanceStatus> {
    const API = requireServerUrl();
    return await backoff(async () => {
        const res = await authedFetch(`${API}/v1/balance/status`);
        if (!res.ok) throw new Error(`balance/status: ${res.status}`);
        return res.json() as Promise<BalanceStatus>;
    });
}

export async function fetchPublicProxyModels(): Promise<{ models: PublicProxyModel[] }> {
    const API = requireServerUrl();
    return await backoff(async () => {
        const res = await fetch(`${API}/v1/balance/models`);
        if (!res.ok) throw new Error(`balance/models: ${res.status}`);
        return res.json() as Promise<{ models: PublicProxyModel[] }>;
    });
}

export async function fetchProxyUsageSummary(
    period: 'today' | '7days' | '30days'
): Promise<ProxyUsageSummary> {
    const API = requireServerUrl();
    const now = Math.floor(Date.now() / 1000);
    let startTime: number;

    switch (period) {
        case 'today': {
            const today = new Date();
            today.setHours(0, 0, 0, 0);
            startTime = Math.floor(today.getTime() / 1000);
            break;
        }
        case '7days':
            startTime = now - 7 * 86400;
            break;
        case '30days':
            startTime = now - 30 * 86400;
            break;
    }

    return await backoff(async () => {
        const res = await authedFetch(`${API}/v1/balance/usage-summary`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({ startTime, endTime: now }),
        });
        if (!res.ok) throw new Error(`usage-summary: ${res.status}`);
        return res.json() as Promise<ProxyUsageSummary>;
    });
}

export async function fetchLedger(
    params: {
        type?: 'usage' | 'topup';
        limit?: number;
        cursor?: string;
    } = {}
): Promise<LedgerResponse> {
    const API = requireServerUrl();
    return await backoff(async () => {
        const res = await authedFetch(`${API}/v1/balance/ledger`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                type: params.type ?? null,
                limit: params.limit ?? 30,
                cursor: params.cursor ?? null,
            }),
        });
        if (!res.ok) throw new Error(`balance/ledger: ${res.status}`);
        return res.json() as Promise<LedgerResponse>;
    });
}
