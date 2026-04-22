import { AuthCredentials } from '@/auth/tokenStorage';
import { backoff } from '@/utils/time';
import { authedFetch } from './authedFetch';
import { requireServerUrl } from './serverConfig';

export interface WechatOAuthParams {
    appId: string;
    url: string;
}

/**
 * Get WeChat OAuth parameters from the server (appId + QR scan URL)
 */
export async function getWechatOAuthParams(_credentials: AuthCredentials): Promise<WechatOAuthParams> {
    const API_ENDPOINT = requireServerUrl();

    return await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/connect/wechat/params`, {
            method: 'GET',
            headers: {
                'Content-Type': 'application/json'
            }
        });

        if (!response.ok) {
            if (response.status === 400) {
                const error = await response.json();
                throw new Error(error.error || 'WeChat OAuth not configured');
            }
            throw new Error(`Failed to get WeChat OAuth params: ${response.status}`);
        }

        return await response.json() as WechatOAuthParams;
    });
}

/**
 * Disconnect WeChat account from the user's profile
 */
export async function disconnectWechat(_credentials: AuthCredentials): Promise<void> {
    const API_ENDPOINT = requireServerUrl();

    return await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/connect/wechat`, {
            method: 'DELETE',
        });

        if (!response.ok) {
            throw new Error(`Failed to disconnect WeChat: ${response.status}`);
        }

        const data = await response.json() as { success: true };
        if (!data.success) {
            throw new Error('Failed to disconnect WeChat account');
        }
    });
}
