/**
 * Cteno 2.0 — Plaintext session payload handler.
 *
 * Replaces the former AES-GCM-backed `SessionEncryption`. Session messages
 * now travel as `{ t: 'plaintext', c: <stringified JSON> }` on the wire, and
 * all metadata / agent-state payloads arrive in the clear too. This class
 * keeps the same method names that the rest of the app already calls so the
 * migration is surgical.
 */
import { RawRecord } from '../typesRaw';
import { ApiMessage } from '../apiTypes';
import {
    DecryptedMessage,
    Metadata,
    MetadataSchema,
    AgentState,
    AgentStateSchema,
} from '../storageTypes';
import { EncryptionCache } from './encryptionCache';

function parseJson<T = unknown>(raw: string): T | null {
    try {
        return JSON.parse(raw) as T;
    } catch {
        return null;
    }
}

export class SessionEncryption {
    private sessionId: string;
    private cache: EncryptionCache;

    constructor(sessionId: string, cache: EncryptionCache) {
        this.sessionId = sessionId;
        this.cache = cache;
    }

    /**
     * Decode a batch of server-supplied messages.  All messages should arrive
     * as `plaintext` in 2.0.  If we somehow encounter a legacy `encrypted`
     * envelope (e.g. because the server was not updated yet) we log and skip —
     * we have no way to decrypt it without the old keys.
     */
    async decryptMessages(messages: ApiMessage[]): Promise<(DecryptedMessage | null)[]> {
        const results: (DecryptedMessage | null)[] = new Array(messages.length);

        for (let i = 0; i < messages.length; i++) {
            const message = messages[i];
            if (!message) {
                results[i] = null;
                continue;
            }

            const cached = this.cache.getCachedMessage(message.id);
            if (cached) {
                results[i] = cached;
                continue;
            }

            let content: unknown = null;
            if (message.content.t === 'plaintext') {
                content = parseJson(message.content.c);
            } else if ((message.content as { t: string }).t === 'encrypted') {
                console.warn(
                    `[sessionEncryption] Dropping legacy encrypted message ${message.id}; server should emit plaintext in 2.0`,
                );
            } else {
                content = null;
            }

            const decoded: DecryptedMessage = {
                id: message.id,
                seq: message.seq,
                localId: message.localId ?? null,
                content,
                createdAt: message.createdAt,
            };
            this.cache.setCachedMessage(message.id, decoded);
            results[i] = decoded;
        }

        return results;
    }

    async decryptMessage(message: ApiMessage | null | undefined): Promise<DecryptedMessage | null> {
        if (!message) {
            return null;
        }
        const [result] = await this.decryptMessages([message]);
        return result;
    }

    async encryptRawRecord(record: RawRecord): Promise<string> {
        return JSON.stringify(record);
    }

    async encryptRaw(data: unknown): Promise<string> {
        return JSON.stringify(data);
    }

    async decryptRaw<T = unknown>(encrypted: string): Promise<T | null> {
        return parseJson<T>(encrypted);
    }

    async encryptMetadata(metadata: Metadata): Promise<string> {
        return JSON.stringify(metadata);
    }

    async decryptMetadata(version: number, encrypted: string): Promise<Metadata | null> {
        const cached = this.cache.getCachedMetadata(this.sessionId, version);
        if (cached) return cached;

        const parsed = MetadataSchema.safeParse(parseJson(encrypted));
        if (!parsed.success) return null;

        this.cache.setCachedMetadata(this.sessionId, version, parsed.data);
        return parsed.data;
    }

    async encryptAgentState(state: AgentState): Promise<string> {
        return JSON.stringify(state);
    }

    async decryptAgentState(
        version: number,
        encrypted: string | null | undefined,
    ): Promise<AgentState> {
        if (!encrypted) return {};

        const cached = this.cache.getCachedAgentState(this.sessionId, version);
        if (cached) return cached;

        const parsed = AgentStateSchema.safeParse(parseJson(encrypted));
        if (!parsed.success) return {};

        this.cache.setCachedAgentState(this.sessionId, version, parsed.data);
        return parsed.data;
    }
}
