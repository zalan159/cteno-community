import * as React from 'react';
import { useAuth } from '@/auth/AuthContext';
import { loginWithBrowserOAuth } from '@/auth/account/authBrowser';
import { Modal } from '@/modal';
import { t } from '@/text';

interface UseConnectAccountOptions {
    onSuccess?: () => void;
    onError?: (error: any) => void;
}

export function useConnectAccount(options?: UseConnectAccountOptions) {
    const auth = useAuth();
    const [isLoading, setIsLoading] = React.useState(false);

    const reloginWithBrowser = React.useCallback(async () => {
        setIsLoading(true);
        try {
            const payload = await loginWithBrowserOAuth();
            await auth.login(payload);
            options?.onSuccess?.();
            return true;
        } catch (error) {
            console.error(error);
            const message = error instanceof Error ? error.message : String(error);
            Modal.alert(t('common.error'), message, [{ text: t('common.ok') }]);
            options?.onError?.(error);
            return false;
        } finally {
            setIsLoading(false);
        }
    }, [auth, options]);

    const processAuthUrl = React.useCallback(async (url: string) => {
        if (url && !url.startsWith('happy:///account?')) {
            Modal.alert(t('common.error'), t('modals.invalidAuthUrl'), [{ text: t('common.ok') }]);
            return false;
        }
        return reloginWithBrowser();
    }, [reloginWithBrowser]);

    const connectAccount = React.useCallback(async () => {
        return reloginWithBrowser();
    }, [reloginWithBrowser]);

    const connectWithUrl = React.useCallback(async (url: string) => {
        return await processAuthUrl(url);
    }, [processAuthUrl]);

    return {
        connectAccount,
        connectWithUrl,
        isLoading,
        processAuthUrl
    };
}
