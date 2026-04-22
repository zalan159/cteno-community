import React from 'react';
import { OAuthViewUnsupported } from '@/components/OAuthView';

export default function ClaudeOAuth() {
    return <OAuthViewUnsupported name="Claude" command="cteno connect claude" />;
}
