import type { VendorName } from '@/sync/ops';

export type PermissionMode =
    | 'default'
    | 'auto'
    | 'acceptEdits'
    | 'plan'
    | 'dontAsk'
    | 'bypassPermissions'
    | 'read-only'
    | 'safe-yolo'
    | 'yolo';

const CTENO_PERMISSION_MODES: PermissionMode[] = [
    'default',
    'acceptEdits',
    'plan',
    'bypassPermissions',
];

const CLAUDE_PERMISSION_MODES: PermissionMode[] = [
    'default',
    'auto',
    'acceptEdits',
    'plan',
    'dontAsk',
    'bypassPermissions',
];

const SANDBOX_PERMISSION_MODES: PermissionMode[] = [
    'default',
    'read-only',
    'safe-yolo',
    'yolo',
];

export function usesSandboxPermissionModes(vendor: VendorName | null | undefined): boolean {
    return vendor === 'codex' || vendor === 'gemini';
}

export function permissionModesForVendor(
    vendor: VendorName | null | undefined,
): PermissionMode[] {
    if (vendor === 'claude') {
        return CLAUDE_PERMISSION_MODES;
    }
    if (usesSandboxPermissionModes(vendor)) {
        return SANDBOX_PERMISSION_MODES;
    }
    return CTENO_PERMISSION_MODES;
}

export function permissionModeOrderForAgent(
    agentType?: 'cteno' | 'claude' | 'codex' | 'gemini',
): PermissionMode[] {
    return permissionModesForVendor(agentType ?? 'cteno');
}

export function permissionModeLabel(
    mode: PermissionMode,
    vendor: VendorName | null | undefined,
): string {
    if (usesSandboxPermissionModes(vendor)) {
        switch (mode) {
            case 'default':
                return 'Ask';
            case 'read-only':
                return 'Read-only';
            case 'safe-yolo':
                return 'Safe YOLO';
            case 'yolo':
                return 'YOLO';
            default:
                return mode;
        }
    }

    switch (mode) {
        case 'default':
            return 'Ask';
        case 'auto':
            return 'Auto';
        case 'acceptEdits':
            return 'Edits';
        case 'plan':
            return 'Plan';
        case 'dontAsk':
            return "Don't Ask";
        case 'bypassPermissions':
            return 'Yolo';
        default:
            return mode;
    }
}

export function permissionModeIcon(
    mode: PermissionMode,
    vendor: VendorName | null | undefined,
): string {
    if (usesSandboxPermissionModes(vendor)) {
        if (mode === 'yolo') return 'flash';
        if (mode === 'safe-yolo') return 'shield-checkmark';
        if (mode === 'read-only') return 'eye';
        return 'shield-checkmark';
    }

    if (mode === 'bypassPermissions') return 'flash';
    if (mode === 'dontAsk') return 'checkmark-done';
    if (mode === 'plan') return 'list';
    if (mode === 'acceptEdits') return 'create';
    if (mode === 'auto') return 'sparkles';
    return 'shield-checkmark';
}
