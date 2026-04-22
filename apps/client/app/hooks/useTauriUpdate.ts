import { useEffect, useRef, useCallback, useState } from 'react';
import { storage } from '@/sync/storage';
import { useShallow } from 'zustand/react/shallow';
import { isTauri } from '@/utils/tauri';
import { getServerUrl, isServerAvailable } from '@/sync/serverConfig';

const CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000; // 4 hours

function getTauriTarget(): string {
    const platform = navigator.platform?.toLowerCase() ?? '';
    if (/win/.test(platform)) return 'windows-x86_64';
    if (/mac/.test(platform)) return 'darwin-aarch64';
    if (/linux/.test(platform)) return 'linux-x86_64';
    return 'unknown';
}

/**
 * Checks for desktop (Tauri) app updates via HTTP fetch (works in both dev and release).
 * Uses Tauri plugin only for the actual download+install step.
 */
export function useTauriUpdate() {
    const status = storage(useShallow((state) => state.desktopUpdateStatus));
    const applyStatus = storage((state) => state.applyDesktopUpdateStatus);
    const updateRef = useRef<any>(null);
    const [error, setError] = useState<string | null>(null);

    const checkForUpdate = useCallback(async () => {
        if (!isTauri()) return;

        try {
            let currentVersion: string;
            try {
                const { getVersion } = await import('@tauri-apps/api/app');
                currentVersion = await getVersion();
            } catch (e) {
                console.warn('[useTauriUpdate] unable to read app version, skip check:', e);
                return;
            }

            const serverUrl = getServerUrl();
            if (!isServerAvailable(serverUrl)) {
                return;
            }

            const target = getTauriTarget();
            const resp = await fetch(`${serverUrl}/v1/desktop/update-check?current_version=${currentVersion}&target=${target}`);
            if (resp.status === 204) {
                applyStatus(null);
                return;
            }
            if (!resp.ok) return;

            const data = await resp.json();
            updateRef.current = data;
            applyStatus({
                available: true,
                version: data.version,
                notes: data.notes ?? undefined,
            });
        } catch (e) {
            console.warn('[useTauriUpdate] check failed:', e);
        }
    }, [applyStatus]);

    const startDownload = useCallback(async () => {
        const data = updateRef.current;
        if (!data) {
            setError('No update data available');
            return;
        }

        setError(null);
        applyStatus({
            available: true,
            version: data.version,
            notes: data.notes ?? undefined,
            downloading: true,
            progress: 0,
        });

        try {
            const { check } = await import('@tauri-apps/plugin-updater');
            const update = await check();
            if (!update) {
                applyStatus({ available: true, version: data.version, downloading: false });
                setError('Updater check returned no update. Try restarting the app.');
                return;
            }

            let downloaded = 0;
            let contentLength: number | undefined;

            await update.downloadAndInstall((event: any) => {
                if (event.event === 'Started') {
                    contentLength = event.data?.contentLength;
                } else if (event.event === 'Progress') {
                    downloaded += event.data.chunkLength;
                    const progress = contentLength
                        ? Math.min(Math.round((downloaded / contentLength) * 100), 99)
                        : undefined;
                    applyStatus({
                        available: true,
                        version: data.version,
                        downloading: true,
                        progress,
                    });
                } else if (event.event === 'Finished') {
                    applyStatus({
                        available: true,
                        version: data.version,
                        downloading: false,
                        progress: 100,
                    });
                }
            });
        } catch (e) {
            const msg = e instanceof Error ? e.message : String(e);
            console.error('[useTauriUpdate] download failed:', msg);
            setError(msg);
            applyStatus({
                available: true,
                version: data.version,
                downloading: false,
            });
        }
    }, [applyStatus]);

    const relaunchApp = useCallback(async () => {
        try {
            const { invoke } = await import('@tauri-apps/api/core');
            await invoke('restart_app');
        } catch {
            window.location.reload();
        }
    }, []);

    useEffect(() => {
        if (!isTauri()) return;

        const initialTimeout = setTimeout(checkForUpdate, 3000);
        const interval = setInterval(checkForUpdate, CHECK_INTERVAL_MS);

        return () => {
            clearTimeout(initialTimeout);
            clearInterval(interval);
        };
    }, [checkForUpdate]);

    return {
        available: status?.available ?? false,
        version: status?.version,
        notes: status?.notes,
        downloading: status?.downloading ?? false,
        progress: status?.progress,
        error,
        startDownload,
        relaunchApp,
    };
}
