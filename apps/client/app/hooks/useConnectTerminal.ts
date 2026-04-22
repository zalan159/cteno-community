import * as React from 'react';
import { useAuth } from '@/auth/AuthContext';
import { loginWithBrowserOAuth } from '@/auth/account/authBrowser';
import { Modal } from '@/modal';
import { t } from '@/text';

interface UseConnectTerminalOptions {
    onSuccess?: () => void;
    onError?: (error: any) => void;
}

export function useConnectTerminal(options?: UseConnectTerminalOptions) {
    const auth = useAuth();
    const [isLoading, setIsLoading] = React.useState(false);
    const showFallbackScanner = false;

    const reloginWithBrowser = React.useCallback(async () => {
        setIsLoading(true);
        try {
            const payload = await loginWithBrowserOAuth();
            await auth.login(payload);
            options?.onSuccess?.();
            return true;
        } catch (error) {
            console.error('[useConnectTerminal] Error:', error);
            const message = error instanceof Error ? error.message : String(error);
            Modal.alert(t('common.error'), message, [{ text: t('common.ok') }]);
            options?.onError?.(error);
            return false;
        } finally {
            setIsLoading(false);
        }
    }, [auth, options]);

    const processAuthUrl = React.useCallback(async (url: string) => {
        if (url && !url.startsWith('happy://terminal?')) {
            console.log('[useConnectTerminal] Invalid URL format');
            Modal.alert(t('common.error'), t('modals.invalidAuthUrl'), [{ text: t('common.ok') }]);
            return false;
        }
        return reloginWithBrowser();
    }, [reloginWithBrowser]);

    const connectTerminal = React.useCallback(async () => {
        return reloginWithBrowser();
    }, [reloginWithBrowser]);

    const connectWithUrl = React.useCallback(async (url: string) => {
        return await processAuthUrl(url);
    }, [processAuthUrl]);

    const handleFallbackScanned = React.useCallback(async (data: string) => {
        await processAuthUrl(data);
    }, [processAuthUrl]);

    const closeFallbackScanner = React.useCallback(() => {}, []);

    return {
        connectTerminal,
        connectWithUrl,
        isLoading,
        processAuthUrl,
        showFallbackScanner,
        handleFallbackScanned,
        closeFallbackScanner,
    };
}
