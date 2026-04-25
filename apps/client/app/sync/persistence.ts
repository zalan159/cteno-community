import { MMKV } from 'react-native-mmkv';
import { Settings, settingsDefaults, settingsParse, SettingsSchema } from './settings';
import { LocalSettings, localSettingsDefaults, localSettingsParse } from './localSettings';
import { LocalProxyUsage, localProxyUsageDefaults, localProxyUsageParse } from './localProxyUsage';
import { Profile, profileDefaults, profileParse } from './profile';
import type { PermissionMode } from '@/components/PermissionModeSelector';
import type { RuntimeEffort } from '@/components/EffortSelector';
import type { VendorName } from './ops';

const mmkv = new MMKV();
const LEGACY_NEW_SESSION_DRAFT_KEY = 'new-session-draft-v1';
const NEW_SESSION_DRAFT_KEY = 'new-session-draft-v2';

export type NewSessionAgentType = VendorName;
export type NewSessionSessionType = 'simple' | 'worktree';

export interface NewSessionDraft {
    input: string;
    selectedMachineId: string | null;
    selectedPath: string | null;
    runtimeVendor: NewSessionAgentType;
    runtimeProfileId: string | null;
    runtimeEffort: RuntimeEffort;
    backendProfileId: string | null;
    permissionMode: PermissionMode;
    sessionType: NewSessionSessionType;
    updatedAt: number;
}

function parseDraft(raw: string | undefined): NewSessionDraft | null {
    if (!raw) {
        return null;
    }
    try {
        const parsed = JSON.parse(raw);
        if (!parsed || typeof parsed !== 'object') {
            return null;
        }

        const input = typeof parsed.input === 'string' ? parsed.input : '';
        const selectedMachineId = typeof parsed.selectedMachineId === 'string' ? parsed.selectedMachineId : null;
        const selectedPath = typeof parsed.selectedPath === 'string' ? parsed.selectedPath : null;
        const runtimeVendor: NewSessionAgentType =
            parsed.runtimeVendor === 'claude' || parsed.runtimeVendor === 'codex' || parsed.runtimeVendor === 'gemini'
                ? parsed.runtimeVendor
                : parsed.agentType === 'claude' || parsed.agentType === 'codex' || parsed.agentType === 'gemini'
                    ? parsed.agentType
                    : 'cteno';
        const runtimeProfileId = typeof parsed.runtimeProfileId === 'string' ? parsed.runtimeProfileId : null;
        const runtimeEffort: RuntimeEffort =
            parsed.runtimeEffort === 'low' || parsed.runtimeEffort === 'medium' || parsed.runtimeEffort === 'high'
                ? parsed.runtimeEffort
                : 'default';
        const backendProfileId = typeof parsed.backendProfileId === 'string' ? parsed.backendProfileId : null;
        const permissionMode: PermissionMode = typeof parsed.permissionMode === 'string'
            ? (parsed.permissionMode as PermissionMode)
            : 'default';
        const sessionType: NewSessionSessionType = parsed.sessionType === 'worktree' ? 'worktree' : 'simple';
        const updatedAt = typeof parsed.updatedAt === 'number' ? parsed.updatedAt : Date.now();

        return {
            input,
            selectedMachineId,
            selectedPath,
            runtimeVendor,
            runtimeProfileId,
            runtimeEffort,
            backendProfileId,
            permissionMode,
            sessionType,
            updatedAt,
        };
    } catch (e) {
        console.error('Failed to parse new session draft', e);
        return null;
    }
}

export function loadSettings(): { settings: Settings, version: number | null } {
    const settings = mmkv.getString('settings');
    if (settings) {
        try {
            const parsed = JSON.parse(settings);
            return { settings: settingsParse(parsed.settings), version: parsed.version };
        } catch (e) {
            console.error('Failed to parse settings', e);
            return { settings: { ...settingsDefaults }, version: null };
        }
    }
    return { settings: { ...settingsDefaults }, version: null };
}

export function saveSettings(settings: Settings, version: number) {
    mmkv.set('settings', JSON.stringify({ settings, version }));
}

export function loadPendingSettings(): Partial<Settings> {
    const pending = mmkv.getString('pending-settings');
    if (pending) {
        try {
            const parsed = JSON.parse(pending);
            return SettingsSchema.partial().parse(parsed);
        } catch (e) {
            console.error('Failed to parse pending settings', e);
            return {};
        }
    }
    return {};
}

export function savePendingSettings(settings: Partial<Settings>) {
    mmkv.set('pending-settings', JSON.stringify(settings));
}

export function loadLocalSettings(): LocalSettings {
    const localSettings = mmkv.getString('local-settings');
    if (localSettings) {
        try {
            const parsed = JSON.parse(localSettings);
            return localSettingsParse(parsed);
        } catch (e) {
            console.error('Failed to parse local settings', e);
            return { ...localSettingsDefaults };
        }
    }
    return { ...localSettingsDefaults };
}

export function saveLocalSettings(settings: LocalSettings) {
    mmkv.set('local-settings', JSON.stringify(settings));
}

export function loadThemePreference(): 'light' | 'dark' | 'adaptive' {
    const localSettings = mmkv.getString('local-settings');
    if (localSettings) {
        try {
            const parsed = JSON.parse(localSettings);
            const settings = localSettingsParse(parsed);
            return settings.themePreference;
        } catch (e) {
            console.error('Failed to parse local settings for theme preference', e);
            return localSettingsDefaults.themePreference;
        }
    }
    return localSettingsDefaults.themePreference;
}

export function loadLocalProxyUsage(): LocalProxyUsage {
    const localProxyUsage = mmkv.getString('local-proxy-usage');
    if (localProxyUsage) {
        try {
            const parsed = JSON.parse(localProxyUsage);
            return localProxyUsageParse(parsed);
        } catch (e) {
            console.error('Failed to parse local proxy usage', e);
            return { ...localProxyUsageDefaults };
        }
    }
    return { ...localProxyUsageDefaults };
}

export function saveLocalProxyUsage(localProxyUsage: LocalProxyUsage) {
    mmkv.set('local-proxy-usage', JSON.stringify(localProxyUsage));
}

export function loadSessionDrafts(): Record<string, string> {
    const drafts = mmkv.getString('session-drafts');
    if (drafts) {
        try {
            return JSON.parse(drafts);
        } catch (e) {
            console.error('Failed to parse session drafts', e);
            return {};
        }
    }
    return {};
}

export function saveSessionDrafts(drafts: Record<string, string>) {
    mmkv.set('session-drafts', JSON.stringify(drafts));
}

export function loadNewSessionDraft(): NewSessionDraft | null {
    return parseDraft(mmkv.getString(NEW_SESSION_DRAFT_KEY))
        ?? parseDraft(mmkv.getString(LEGACY_NEW_SESSION_DRAFT_KEY));
}

export function saveNewSessionDraft(draft: NewSessionDraft) {
    mmkv.set(NEW_SESSION_DRAFT_KEY, JSON.stringify(draft));
}

export function clearNewSessionDraft() {
    mmkv.delete(NEW_SESSION_DRAFT_KEY);
}

export function loadSessionPermissionModes(): Record<string, PermissionMode> {
    const modes = mmkv.getString('session-permission-modes');
    if (modes) {
        try {
            return JSON.parse(modes);
        } catch (e) {
            console.error('Failed to parse session permission modes', e);
            return {};
        }
    }
    return {};
}

export function saveSessionPermissionModes(modes: Record<string, PermissionMode>) {
    mmkv.set('session-permission-modes', JSON.stringify(modes));
}

export function loadSessionRuntimeEfforts(): Record<string, RuntimeEffort> {
    const efforts = mmkv.getString('session-runtime-efforts');
    if (efforts) {
        try {
            return JSON.parse(efforts);
        } catch (e) {
            console.error('Failed to parse session runtime efforts', e);
            return {};
        }
    }
    return {};
}

export function saveSessionRuntimeEfforts(efforts: Record<string, RuntimeEffort>) {
    mmkv.set('session-runtime-efforts', JSON.stringify(efforts));
}

export function loadSessionSandboxPolicies(): Record<string, string> {
    const policies = mmkv.getString('session-sandbox-policies');
    if (policies) {
        try {
            return JSON.parse(policies);
        } catch (e) {
            console.error('Failed to parse session sandbox policies', e);
            return {};
        }
    }
    return {};
}

export function saveSessionSandboxPolicies(policies: Record<string, string>) {
    mmkv.set('session-sandbox-policies', JSON.stringify(policies));
}

export function loadProfile(): Profile {
    const profile = mmkv.getString('profile');
    if (profile) {
        try {
            const parsed = JSON.parse(profile);
            return profileParse(parsed);
        } catch (e) {
            console.error('Failed to parse profile', e);
            return { ...profileDefaults };
        }
    }
    return { ...profileDefaults };
}

export function saveProfile(profile: Profile) {
    mmkv.set('profile', JSON.stringify(profile));
}

// Simple temporary text storage for passing large strings between screens
export function storeTempText(content: string): string {
    const id = `temp_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`;
    mmkv.set(`temp_text_${id}`, content);
    return id;
}

export function retrieveTempText(id: string): string | null {
    const content = mmkv.getString(`temp_text_${id}`);
    if (content) {
        // Auto-delete after retrieval
        mmkv.delete(`temp_text_${id}`);
        return content;
    }
    return null;
}

export function loadPersonaReadTimestamps(): Record<string, number> {
    const data = mmkv.getString('persona-read-timestamps');
    if (data) {
        try {
            return JSON.parse(data);
        } catch (e) {
            console.error('Failed to parse persona read timestamps', e);
            return {};
        }
    }
    return {};
}

export function savePersonaReadTimestamps(timestamps: Record<string, number>) {
    mmkv.set('persona-read-timestamps', JSON.stringify(timestamps));
}

export function loadCachedPersonas(): any[] {
    const data = mmkv.getString('cached-personas');
    if (data) {
        try {
            return JSON.parse(data);
        } catch {
            // ignore
        }
    }
    return [];
}

export function saveCachedPersonas(personas: any[]) {
    mmkv.set('cached-personas', JSON.stringify(personas));
}

export function loadCachedPersonaProjects(): any[] {
    const data = mmkv.getString('cached-persona-projects');
    if (data) {
        try {
            return JSON.parse(data);
        } catch {
            // ignore
        }
    }
    return [];
}

export function saveCachedPersonaProjects(projects: any[]) {
    mmkv.set('cached-persona-projects', JSON.stringify(projects));
}

export function loadCachedAgentWorkspaces(): any[] {
    const data = mmkv.getString('cached-agent-workspaces');
    if (data) {
        try {
            return JSON.parse(data);
        } catch {
            // ignore
        }
    }
    return [];
}

export function saveCachedAgentWorkspaces(workspaces: any[]) {
    mmkv.set('cached-agent-workspaces', JSON.stringify(workspaces));
}

export function clearPersistence() {
    mmkv.clearAll();
}
