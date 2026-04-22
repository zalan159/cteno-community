import { useEffect, useState, useCallback, useRef } from 'react';
import { AppState, AppStateStatus, Platform } from 'react-native';
import * as Updates from 'expo-updates';

export function useUpdates() {
    const [updateAvailable, setUpdateAvailable] = useState(false);
    const isCheckingRef = useRef(false);

    const checkForUpdates = useCallback(async () => {
        if (__DEV__) return;
        if (isCheckingRef.current) return;
        isCheckingRef.current = true;

        try {
            const update = await Updates.checkForUpdateAsync();
            if (update.isAvailable) {
                await Updates.fetchUpdateAsync();
                setUpdateAvailable(true);
            }
        } catch (error: any) {
            console.error('Error checking for updates:', error);
        } finally {
            isCheckingRef.current = false;
        }
    }, []);

    useEffect(() => {
        const subscription = AppState.addEventListener('change', (nextAppState: AppStateStatus) => {
            if (nextAppState === 'active') {
                checkForUpdates();
            }
        });

        // Initial check after short delay
        const timer = setTimeout(checkForUpdates, 2000);

        return () => {
            subscription.remove();
            clearTimeout(timer);
        };
    }, [checkForUpdates]);

    const reloadApp = useCallback(async () => {
        if (Platform.OS === 'web') {
            window.location.reload();
        } else {
            try {
                await Updates.reloadAsync();
            } catch (error) {
                console.error('Error reloading app:', error);
            }
        }
    }, []);

    return {
        updateAvailable,
        isChecking: isCheckingRef.current,
        checkForUpdates,
        reloadApp,
    };
}