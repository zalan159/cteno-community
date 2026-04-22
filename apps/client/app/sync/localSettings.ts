import * as z from 'zod';

//
// Schema
//

export const LocalSettingsSchema = z.object({
    // Developer settings (device-specific)
    debugMode: z.boolean().describe('Enable debug logging'),
    devModeEnabled: z.boolean().describe('Enable developer menu in settings'),
    commandPaletteEnabled: z.boolean().describe('Enable CMD+K command palette (web only)'),
    themePreference: z.enum(['light', 'dark', 'adaptive']).describe('Theme preference: light, dark, or adaptive (follows system)'),
    markdownCopyV2: z.boolean().describe('Replace native paragraph selection with long-press modal for full markdown copy'),
    // CLI version acknowledgments - keyed by machineId
    acknowledgedCliVersions: z.record(z.string(), z.string()).describe('Acknowledged CLI versions per machine'),
    // Selected machine filter for session list (null = show all machines)
    selectedMachineIdFilter: z.string().nullable().describe('Filter sessions by machine ID, null shows all'),
    // Skip continuous browsing confirmation dialog
    skipContinuousBrowsingConfirm: z.boolean().describe('Skip the confirmation dialog when enabling continuous browsing'),
    // Preferred desktop sidebar width (null = responsive default)
    desktopSidebarWidth: z.number().nullable().describe('Preferred desktop sidebar width in pixels'),
    // Local-only first-launch wizard completion timestamp (ms).
    // 0 = not completed yet, >0 = completed at this time.
    localSetupCompletedAt: z.number().describe('Local-first setup wizard completion timestamp (0 = not completed)'),
    // Once true, the desktop session filter has been initialized to the
    // local machine on first launch. Prevents re-defaulting on every app
    // start — user's explicit "All devices" / other-machine picks persist.
    defaultedToLocalMachine: z.boolean().describe('Filter already initialized to local machine on first launch'),
});

//
// NOTE: Local settings are device-specific and should NOT be synced.
// These are preferences that make sense to be different on each device.
//

const LocalSettingsSchemaPartial = LocalSettingsSchema.passthrough().partial();

export type LocalSettings = z.infer<typeof LocalSettingsSchema>;

//
// Defaults
//

export const localSettingsDefaults: LocalSettings = {
    debugMode: false,
    devModeEnabled: false,
    commandPaletteEnabled: false,
    themePreference: 'adaptive',
    markdownCopyV2: false,
    acknowledgedCliVersions: {},
    selectedMachineIdFilter: null, // Show all machines by default
    skipContinuousBrowsingConfirm: false,
    desktopSidebarWidth: null,
    localSetupCompletedAt: 0,
    defaultedToLocalMachine: false,
};
Object.freeze(localSettingsDefaults);

//
// Parsing
//

export function localSettingsParse(settings: unknown): LocalSettings {
    const parsed = LocalSettingsSchemaPartial.safeParse(settings);
    if (!parsed.success) {
        return { ...localSettingsDefaults };
    }
    return { ...localSettingsDefaults, ...parsed.data };
}

//
// Applying changes
//

export function applyLocalSettings(settings: LocalSettings, delta: Partial<LocalSettings>): LocalSettings {
    return { ...localSettingsDefaults, ...settings, ...delta };
}
