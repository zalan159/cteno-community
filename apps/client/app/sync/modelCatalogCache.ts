import { MMKV } from 'react-native-mmkv';

const mmkv = new MMKV({ id: 'model-catalog-cache' });
const KEY = 'vendor-model-catalogs-v4';

export type CachedVendorName = 'cteno' | 'claude' | 'codex' | 'gemini';

export interface CachedVendorModelCatalog {
    machineId: string;
    vendor: CachedVendorName;
    models: any[];
    defaultModelId: string;
    cachedAt: number;
}

type CacheMap = Record<string, CachedVendorModelCatalog>;

function cacheKey(machineId: string, vendor: CachedVendorName): string {
    return `${machineId}:${vendor}`;
}

function loadAll(): CacheMap {
    const raw = mmkv.getString(KEY);
    if (!raw) {
        return {};
    }
    try {
        const parsed = JSON.parse(raw);
        return parsed && typeof parsed === 'object' ? parsed as CacheMap : {};
    } catch (error) {
        console.error('Failed to parse model catalog cache', error);
        return {};
    }
}

function saveAll(value: CacheMap) {
    mmkv.set(KEY, JSON.stringify(value));
}

export function loadCachedVendorModelCatalog(
    machineId: string,
    vendor: CachedVendorName,
): CachedVendorModelCatalog | null {
    return loadAll()[cacheKey(machineId, vendor)] ?? null;
}

export function saveCachedVendorModelCatalog(
    catalog: CachedVendorModelCatalog,
) {
    const next = loadAll();
    next[cacheKey(catalog.machineId, catalog.vendor)] = catalog;
    saveAll(next);
}

export function loadCachedVendorDefaultModelId(
    machineId: string,
    vendor: CachedVendorName,
): string | null {
    return loadCachedVendorModelCatalog(machineId, vendor)?.defaultModelId ?? null;
}
