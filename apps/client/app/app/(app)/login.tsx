import React from 'react';
import { useRouter } from 'expo-router';

import { BrowserAuthEntry } from '@/auth/account';
import { useAuth } from '@/auth/AuthContext';
import { shouldUseLocalTokenLogin } from '@/config/capabilities';

export default function Login() {
    const router = useRouter();
    const auth = useAuth();

    // Once auth.login() flips isAuthenticated, leave this page; the root
    // route owner (app/(app)/index.tsx) renders the signed-in Main view.
    // We intentionally watch `isAuthenticated` (not `hasAppAccess`) so that
    // local-mode users — who already have `hasAppAccess=true` — can still
    // navigate into /login to sign in.
    React.useEffect(() => {
        if (auth.isAuthenticated) {
            if (router.canGoBack()) {
                router.back();
            } else {
                router.replace('/');
            }
        }
    }, [auth.isAuthenticated, router]);

    const localTokenLogin = shouldUseLocalTokenLogin();

    return (
        <BrowserAuthEntry
            title="登录 / 注册"
            subtitle={
                localTokenLogin
                    ? '登录后仅用于 Cteno agent 内置模型鉴权，本地模式不会启用云同步。'
                    : '登录 Cteno 账号以使用内置模型、多端同步与代付功能。'
            }
            loginMode={localTokenLogin ? 'local-token' : 'cloud'}
        />
    );
}
