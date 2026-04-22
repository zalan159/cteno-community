import { useEffect, useState } from 'react';
import { useSession } from '@/sync/storage';
import {
    AgentCapabilities,
    RuntimeControlCapability,
    VendorMeta,
    VendorName,
    machineGetSession,
    listAvailableVendors,
} from '@/sync/ops';
import { Session } from '@/sync/storageTypes';

const KNOWN_VENDORS: VendorName[] = ['cteno', 'claude', 'codex', 'gemini'];

function parseVendorName(value: string | null | undefined): VendorName | null {
    if (!value) {
        return null;
    }
    const normalized = value.trim().toLowerCase();
    return KNOWN_VENDORS.includes(normalized as VendorName) ? normalized as VendorName : null;
}

function getAuthoritativeSessionVendor(session: Session | null | undefined): VendorName | null {
    return parseVendorName(session?.metadata?.vendor);
}

/**
 * Infer the executor vendor a session was spawned against.
 *
 * Session snapshots still expose the executor as `metadata.flavor`, so the
 * hook keeps a best-effort parser here until the backend ships a first-class
 * vendor field.
 */
export function inferSessionVendor(session: Session | null | undefined): VendorName {
    const vendor = getAuthoritativeSessionVendor(session);
    if (vendor) return vendor;

    const flavor = session?.metadata?.flavor?.toLowerCase() ?? '';
    if (flavor.includes('codex')) return 'codex';
    if (flavor.includes('claude')) return 'claude';
    if (flavor.includes('gemini')) return 'gemini';
    if (flavor.includes('cteno')) return 'cteno';
    return 'cteno';
}

function capabilityCacheKey(machineId: string | null | undefined, vendor: VendorName): string {
    return `${machineId ?? 'global'}:${vendor}`;
}

const CAPABILITY_CACHE = new Map<string, VendorMeta>();

const UNSUPPORTED_CONTROL: RuntimeControlCapability = {
    outcome: 'unsupported',
    reason: null,
};

export interface SessionRuntimeControls {
    vendor: VendorName | null;
    capabilities: AgentCapabilities;
    model: RuntimeControlCapability;
    permissionMode: RuntimeControlCapability;
    sandboxPolicySupported: boolean;
    loaded: boolean;
}

function resolveRuntimeControls(
    vendor: VendorName | null,
    capabilities: AgentCapabilities | null,
    unresolvedReason?: string | null,
): SessionRuntimeControls {
    const resolvedCapabilities = capabilities ?? {};
    const unsupportedReason = unresolvedReason ?? null;
    return {
        vendor,
        capabilities: resolvedCapabilities,
        model: resolvedCapabilities.runtimeControls?.model ?? {
            ...UNSUPPORTED_CONTROL,
            reason: unsupportedReason,
        },
        permissionMode: resolvedCapabilities.runtimeControls?.permissionMode ?? {
            ...UNSUPPORTED_CONTROL,
            reason: unsupportedReason,
        },
        sandboxPolicySupported: resolvedCapabilities.setSandboxPolicy === true,
        loaded: vendor !== null && capabilities !== null,
    };
}

export function useSessionRuntimeControls(sessionId: string | null | undefined): SessionRuntimeControls {
    const session = useSession(sessionId ?? '');
    const machineId = session?.metadata?.machineId ?? null;
    const metadataVendor = getAuthoritativeSessionVendor(session);
    const [vendor, setVendor] = useState<VendorName | null>(() => metadataVendor);
    const cacheKey = vendor ? capabilityCacheKey(machineId, vendor) : null;
    const [meta, setMeta] = useState<VendorMeta | null>(() => {
        if (!cacheKey) {
            return null;
        }
        return CAPABILITY_CACHE.get(cacheKey) ?? null;
    });

    useEffect(() => {
        setVendor(metadataVendor ?? null);
    }, [metadataVendor]);

    useEffect(() => {
        if (metadataVendor || !machineId || !sessionId) {
            return;
        }

        let cancelled = false;
        machineGetSession(machineId, sessionId)
            .then((hostSession) => {
                if (cancelled) return;
                const resolvedVendor = getAuthoritativeSessionVendor(hostSession);
                if (resolvedVendor) {
                    setVendor(resolvedVendor);
                }
            })
            .catch(() => {
                /* ignored — runtime controls remain unresolved until metadata exposes a vendor */
            });

        return () => {
            cancelled = true;
        };
    }, [machineId, metadataVendor, sessionId]);

    useEffect(() => {
        if (!cacheKey) {
            setMeta(null);
            return;
        }
        setMeta(CAPABILITY_CACHE.get(cacheKey) ?? null);
    }, [cacheKey]);

    useEffect(() => {
        if (!vendor || !cacheKey) {
            return;
        }

        let cancelled = false;

        listAvailableVendors(machineId)
            .then((vendors) => {
                if (cancelled) return;
                for (const next of vendors) {
                    CAPABILITY_CACHE.set(capabilityCacheKey(machineId, next.name), next);
                }
                setMeta(CAPABILITY_CACHE.get(cacheKey) ?? null);
            })
            .catch(() => {
                /* ignored — unresolved capability stays unsupported */
            });

        return () => {
            cancelled = true;
        };
    }, [cacheKey, machineId, vendor]);

    const unresolvedReason = vendor
        ? null
        : machineId
            ? 'Resolving runtime controls from the attached machine.'
            : 'This session does not expose an executor vendor.';

    return resolveRuntimeControls(vendor, meta?.capabilities ?? null, unresolvedReason);
}

/**
 * Backward-compatible boolean view used by older call sites.
 */
export function useCapability(
    sessionId: string | null | undefined,
    capability: keyof AgentCapabilities | string,
): boolean {
    const runtimeControls = useSessionRuntimeControls(sessionId);
    if (capability === 'setModel') {
        return runtimeControls.model.outcome !== 'unsupported';
    }
    if (capability === 'setPermissionMode') {
        return runtimeControls.permissionMode.outcome !== 'unsupported';
    }
    if (capability === 'setSandboxPolicy') {
        return runtimeControls.sandboxPolicySupported;
    }
    return runtimeControls.capabilities[capability] === true;
}

/**
 * Convenience helper — returns a short human-readable reason for why a
 * capability is unavailable for the given session.
 */
export function useCapabilityDisabledReason(
    sessionId: string | null | undefined,
    capability: keyof AgentCapabilities | string,
): string | null {
    const runtimeControls = useSessionRuntimeControls(sessionId);
    const vendorLabel = runtimeControls.vendor ? `${runtimeControls.vendor} executor` : 'session executor';

    if (capability === 'setModel') {
        return runtimeControls.model.outcome === 'unsupported'
            ? (runtimeControls.model.reason ?? `Not supported by the ${vendorLabel}.`)
            : null;
    }
    if (capability === 'setPermissionMode') {
        return runtimeControls.permissionMode.outcome === 'unsupported'
            ? (runtimeControls.permissionMode.reason ?? `Not supported by the ${vendorLabel}.`)
            : null;
    }
    if (capability === 'setSandboxPolicy' && !runtimeControls.sandboxPolicySupported) {
        return `Not supported by the ${vendorLabel}.`;
    }
    return null;
}
