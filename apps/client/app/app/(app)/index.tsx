import { useAuth } from "@/auth/AuthContext";
import * as React from 'react';
import { MainView } from "@/components/MainView";
import { BrowserAuthEntry } from '@/auth/account';

export default function Home() {
    const auth = useAuth();
    if (!auth.hasAppAccess) {
        return <BrowserAuthEntry />;
    }
    return <MainView variant="phone" />;
}
