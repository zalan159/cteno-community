import type { Metadata } from '@/sync/storageTypes';
import type { ToolCall } from '@/sync/typesMessage';
import { isHostOwnedTool } from '@/components/tools/hostTool';

function isShellLikeTool(tool: Pick<ToolCall, 'name'>): boolean {
    return tool.name === 'Bash' || tool.name === 'shell' || tool.name === 'zsh' || tool.name === 'bash';
}

function usesVendorManagedRuntime(metadata: Metadata | null | undefined): boolean {
    const vendor = metadata?.vendor?.trim().toLowerCase();
    if (vendor === 'cteno' || vendor === 'claude' || vendor === 'codex' || vendor === 'gemini') {
        return true;
    }

    const flavor = metadata?.flavor?.trim().toLowerCase() ?? '';
    return ['cteno', 'claude', 'codex', 'gemini', 'persona'].some((marker) => flavor.includes(marker));
}

export function supportsHostBackgroundTransfer(
    metadata: Metadata | null | undefined,
    tool: Pick<ToolCall, 'name' | 'state' | 'callId' | 'input'>,
): boolean {
    if (!isShellLikeTool(tool) || tool.state !== 'running' || !tool.callId) {
        return false;
    }
    if (isHostOwnedTool(tool)) {
        return true;
    }
    return !usesVendorManagedRuntime(metadata);
}
