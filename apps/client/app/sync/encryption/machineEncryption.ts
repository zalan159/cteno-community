/**
 * Cteno 2.0 — Plaintext machine payload handler.
 *
 * Replaces the former AES-GCM-backed `MachineEncryption`. Machine metadata
 * and daemon-state now arrive unencrypted from the server so this class is a
 * thin JSON-parse wrapper kept around for call-site compatibility.
 */
import { MachineMetadata, MachineMetadataSchema } from '../storageTypes';
import { EncryptionCache } from './encryptionCache';

function parseJson<T = unknown>(raw: string): T | null {
    try {
        return JSON.parse(raw) as T;
    } catch {
        return null;
    }
}

export class MachineEncryption {
    private machineId: string;
    private cache: EncryptionCache;

    constructor(machineId: string, cache: EncryptionCache) {
        this.machineId = machineId;
        this.cache = cache;
    }

    async encryptMetadata(metadata: MachineMetadata): Promise<string> {
        return JSON.stringify(metadata);
    }

    async decryptMetadata(version: number, encrypted: string): Promise<MachineMetadata | null> {
        const cached = this.cache.getCachedMachineMetadata(this.machineId, version);
        if (cached) return cached;

        const parsed = MachineMetadataSchema.safeParse(parseJson(encrypted));
        if (!parsed.success) return null;

        this.cache.setCachedMachineMetadata(this.machineId, version, parsed.data);
        return parsed.data;
    }

    async encryptDaemonState(state: unknown): Promise<string> {
        return JSON.stringify(state);
    }

    async decryptDaemonState(
        version: number,
        encrypted: string | null | undefined,
    ): Promise<unknown | null> {
        if (!encrypted) return null;

        const cached = this.cache.getCachedDaemonState(this.machineId, version);
        if (cached !== undefined) return cached;

        const result = parseJson(encrypted);
        this.cache.setCachedDaemonState(this.machineId, version, result);
        return result;
    }

    async encryptRaw(data: unknown): Promise<string> {
        return JSON.stringify(data);
    }

    async decryptRaw<T = unknown>(encrypted: string): Promise<T | null> {
        return parseJson<T>(encrypted);
    }
}
