import { ToolCall } from '@/sync/typesMessage';

export const HOST_OWNED_TOOL_METADATA_KEY = '__cteno_host';

export interface HostToolMetadata {
    owned: true;
    requestId: string | null;
    source: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function getHostToolMetadata(tool: Pick<ToolCall, 'input'>): HostToolMetadata | null {
    const raw = tool.input?.[HOST_OWNED_TOOL_METADATA_KEY];
    if (!isRecord(raw) || raw.owned !== true) {
        return null;
    }

    return {
        owned: true,
        requestId: typeof raw.requestId === 'string' ? raw.requestId : null,
        source: typeof raw.source === 'string' ? raw.source : null,
    };
}

export function isHostOwnedTool(tool: Pick<ToolCall, 'input'>): boolean {
    return getHostToolMetadata(tool) !== null;
}

export function getHostToolSubtitle(
    tool: Pick<ToolCall, 'input'>,
    currentSubtitle?: string | null,
): string | null {
    if (typeof currentSubtitle === 'string' && currentSubtitle.trim().length > 0) {
        return currentSubtitle;
    }
    if (!isHostOwnedTool(tool)) {
        return null;
    }
    return 'Triggered by host runtime';
}
