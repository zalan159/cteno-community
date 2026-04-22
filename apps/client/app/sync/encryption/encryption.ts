/**
 * Cteno 2.0 — Plaintext stub replacing the former end-to-end encryption layer.
 *
 * Prior to 2.0 this module managed per-session / per-machine AES-GCM keys
 * derived from a shared NaCl keypair. In 2.0 the server stores and ships
 * everything in plaintext (see `SessionMessageContent = { t: 'plaintext', c }`
 * on the server), so all that remains is a compatibility shim preserving the
 * previous public API surface. Every method is a no-op or a JSON pass-through.
 *
 * We deliberately keep the shape of `Encryption.create`, `initializeSessions`,
 * `getSessionEncryption`, etc. because several files still reference them —
 * migrating them incrementally would balloon this PR. The shim is trivially
 * cheap and documents that "this code path used to encrypt".
 */
import { randomUUID } from 'expo-crypto';
import { SessionEncryption } from './sessionEncryption';
import { MachineEncryption } from './machineEncryption';
import { EncryptionCache } from './encryptionCache';

export class Encryption {
    /**
     * Kept for API compat. `masterSecret` is ignored — everything is plaintext.
     */
    static async create(_masterSecret?: Uint8Array | null): Promise<Encryption> {
        return new Encryption();
    }

    /**
     * Anonymous analytics ID.  Previously derived from the master secret;
     * now a random per-process UUID. Analytics callers only use it as an
     * opaque string, so stability across launches is not required.
     */
    readonly anonID: string;

    private sessionEncryptions = new Map<string, SessionEncryption>();
    private machineEncryptions = new Map<string, MachineEncryption>();
    private cache: EncryptionCache;

    constructor() {
        this.anonID = randomUUID().replace(/-/g, '').slice(0, 16).toLowerCase();
        this.cache = new EncryptionCache();
    }

    //
    // Session operations — always create a plaintext SessionEncryption.
    //

    async initializeSessions(sessions: Map<string, Uint8Array | null>): Promise<void> {
        for (const [sessionId] of sessions) {
            if (this.sessionEncryptions.has(sessionId)) continue;
            this.sessionEncryptions.set(
                sessionId,
                new SessionEncryption(sessionId, this.cache),
            );
        }
    }

    getSessionEncryption(sessionId: string): SessionEncryption | null {
        let encryption = this.sessionEncryptions.get(sessionId);
        if (!encryption) {
            // In plaintext mode we can lazily mint a SessionEncryption on
            // demand — there is no secret material to worry about.
            encryption = new SessionEncryption(sessionId, this.cache);
            this.sessionEncryptions.set(sessionId, encryption);
        }
        return encryption;
    }

    async reinitializeSession(sessionId: string, _dataKey: Uint8Array): Promise<void> {
        this.sessionEncryptions.set(
            sessionId,
            new SessionEncryption(sessionId, this.cache),
        );
        this.cache.clearSessionCache(sessionId);
    }

    removeSessionEncryption(sessionId: string): void {
        this.sessionEncryptions.delete(sessionId);
        this.cache.clearSessionCache(sessionId);
    }

    //
    // Machine operations
    //

    async initializeMachines(machines: Map<string, Uint8Array | null>): Promise<void> {
        for (const [machineId] of machines) {
            if (this.machineEncryptions.has(machineId)) continue;
            this.machineEncryptions.set(
                machineId,
                new MachineEncryption(machineId, this.cache),
            );
        }
    }

    getMachineEncryption(machineId: string): MachineEncryption | null {
        let encryption = this.machineEncryptions.get(machineId);
        if (!encryption) {
            encryption = new MachineEncryption(machineId, this.cache);
            this.machineEncryptions.set(machineId, encryption);
        }
        return encryption;
    }

    //
    // Legacy raw-payload helpers. In plaintext mode "encrypt" means
    // "serialize" and "decrypt" means "parse".
    //

    async encryptRaw(data: unknown): Promise<string> {
        return JSON.stringify(data);
    }

    async decryptRaw<T = unknown>(encrypted: string): Promise<T | null> {
        try {
            return JSON.parse(encrypted) as T;
        } catch {
            return null;
        }
    }

    //
    // Data-encryption-key helpers — no-ops preserved for call compat.
    // `decryptEncryptionKey` used to return a Uint8Array AES key; nothing
    // consumes the bytes meaningfully anymore, so returning a zero-length
    // array is safe.  All call sites fall back to plaintext mode when a
    // null value is returned; we return a non-null Uint8Array so the
    // "skip this session" code path does not incorrectly fire.
    //

    async decryptEncryptionKey(_encrypted: string): Promise<Uint8Array | null> {
        return new Uint8Array(0);
    }

    async encryptEncryptionKey(key: Uint8Array): Promise<Uint8Array> {
        return key;
    }

    generateId(): string {
        return randomUUID();
    }
}
