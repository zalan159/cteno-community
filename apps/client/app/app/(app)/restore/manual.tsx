import React from 'react';
import { BrowserAuthEntry } from '@/auth/account';

export default function RestoreManual() {
    return (
        <BrowserAuthEntry
            title="Restore Access"
            subtitle="Use browser sign-in to restore access on this device. Manual secret entry is no longer supported."
            buttonTitle="Sign In"
        />
    );
}
