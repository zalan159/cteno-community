import type { VendorName } from '@/sync/ops';

export const VENDOR_ICON_IMAGES: Record<VendorName, number> = {
    cteno: require('@/assets/images/icon.png'),
    claude: require('@/assets/images/icon-claude.png'),
    codex: require('@/assets/images/icon-codex.png'),
    gemini: require('@/assets/images/icon-gemini.png'),
};

export function getVendorIconSource(vendor: VendorName | string | null | undefined): number {
    switch ((vendor || '').toLowerCase()) {
        case 'claude':
            return VENDOR_ICON_IMAGES.claude;
        case 'codex':
            return VENDOR_ICON_IMAGES.codex;
        case 'gemini':
            return VENDOR_ICON_IMAGES.gemini;
        case 'cteno':
        default:
            return VENDOR_ICON_IMAGES.cteno;
    }
}

export function getVendorAvatarId(vendor: VendorName | string | null | undefined): string {
    const normalized = (vendor || 'cteno').toLowerCase();
    return `vendor:${normalized}`;
}

export function isVendorAvatarId(id: string | null | undefined): boolean {
    return typeof id === 'string' && id.startsWith('vendor:');
}

export function getVendorFromAvatarId(id: string | null | undefined): VendorName | null {
    if (!isVendorAvatarId(id)) {
        return null;
    }
    const vendor = id!.slice('vendor:'.length).toLowerCase();
    if (vendor === 'claude' || vendor === 'codex' || vendor === 'gemini' || vendor === 'cteno') {
        return vendor;
    }
    return null;
}
