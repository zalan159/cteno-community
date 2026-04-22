import { AuthCredentials } from '@/auth/tokenStorage';
import { authedFetch } from './authedFetch';
import { requireServerUrl } from './serverConfig';

export interface NlsTokenResponse {
    allowed: boolean;
    token?: string;
    appkey?: string;
    expireTime?: number;
}

export async function fetchNlsToken(
    _credentials: AuthCredentials
): Promise<NlsTokenResponse> {
    const serverUrl = requireServerUrl();

    const response = await authedFetch(`${serverUrl}/v1/voice/token`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json'
        },
        body: JSON.stringify({})
    });

    if (!response.ok) {
        if (response.status === 400) {
            return { allowed: false };
        }
        throw new Error(`NLS token request failed: ${response.status}`);
    }

    return await response.json();
}
