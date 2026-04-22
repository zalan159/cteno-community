/**
 * Legacy debug helpers — kept for dev-console compatibility.
 *
 * Cteno 2.0 no longer uses end-to-end encryption on the Expo client, so this
 * file is essentially a no-op.  We preserve the global `debugEncryption` /
 * `debugSettings` bindings to avoid breaking hand-typed debug workflows.
 */
import { sync } from '@/sync/sync';

export interface EncryptionDebugInfo {
    hasEncryption: boolean;
    plaintext: true;
    anonID: string;
}

export async function getEncryptionDebugInfo(): Promise<EncryptionDebugInfo> {
    const encryption = (sync as any).encryption;
    return {
        hasEncryption: !!encryption,
        plaintext: true,
        anonID: encryption?.anonID ?? 'unknown',
    };
}

export async function debugEncryptionToConsole() {
    console.log('=== Encryption Debug Info (plaintext mode) ===');
    const info = await getEncryptionDebugInfo();
    console.log('Plaintext mode:', info.plaintext);
    console.log('anonID:', info.anonID);
    console.log('==============================================');
    return info;
}

export async function debugSettingsToConsole() {
    console.log('=== Settings Debug ===');

    try {
        const storage = (window as any).__storage;
        if (!storage) {
            console.error('Storage not found');
            return null;
        }

        const state = storage.getState();
        const settings = state.settings;
        const profiles = settings?.profiles || [];

        console.log('Settings loaded:', !!settings);
        console.log('Settings version:', state.settingsVersion);
        console.log('Settings.profiles count:', profiles.length);

        profiles.forEach((profile: any, index: number) => {
            console.log(`  [${index}] ${profile.name}:`, {
                id: profile.id,
                baseURL: profile.baseURL || 'default',
                chatModel: profile.chatModel,
                hasAuthToken: !!profile.authToken,
            });
        });

        console.log('Last used profile:', settings?.lastUsedProfile || 'none');
        console.log('======================');

        return {
            hasSettings: !!settings,
            settingsProfilesCount: profiles.length,
            settingsProfiles: profiles.map((p: any) => ({ id: p.id, name: p.name })),
        };
    } catch (error) {
        console.error('Error debugging settings:', error);
        return null;
    }
}

if (__DEV__) {
    (window as any).debugEncryption = debugEncryptionToConsole;
    (window as any).debugSettings = debugSettingsToConsole;
}
