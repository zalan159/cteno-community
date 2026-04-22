import { useCallback, useEffect, useState } from 'react';

import { useAuth } from '@/auth/AuthContext';
import { type BalanceStatus, fetchBalanceStatus } from '@/sync/apiBalance';

interface UseBalanceStatusResult {
    balance: BalanceStatus | null;
    loading: boolean;
    error: string | null;
    refresh: () => Promise<void>;
}

export function useBalanceStatus(): UseBalanceStatusResult {
    const { credentials } = useAuth();
    const [balance, setBalance] = useState<BalanceStatus | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(async () => {
        if (!credentials) {
            setBalance(null);
            setError(null);
            setLoading(false);
            return;
        }

        try {
            setError(null);
            setBalance(await fetchBalanceStatus());
        } catch (e) {
            setError(e instanceof Error ? e.message : 'Unknown error');
        } finally {
            setLoading(false);
        }
    }, [credentials]);

    useEffect(() => {
        setLoading(true);
        void refresh();
        const interval = setInterval(() => {
            void refresh();
        }, 60_000);
        return () => clearInterval(interval);
    }, [refresh]);

    return {
        balance,
        loading,
        error,
        refresh,
    };
}
