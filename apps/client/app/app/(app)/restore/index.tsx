import React from 'react';
import { BrowserAuthEntry } from '@/auth/account';

export default function Restore() {
    return (
        <BrowserAuthEntry
            title="Restore Access"
            subtitle="To restore this app on this device, sign in again in your browser. Secret-key restore is no longer supported."
            buttonTitle="Sign In"
        />
    );
}
