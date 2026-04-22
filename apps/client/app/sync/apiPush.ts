import { AuthCredentials } from '@/auth/tokenStorage';
import { backoff } from '@/utils/time';
import { authedFetch } from './authedFetch';
import { requireServerUrl } from './serverConfig';

export async function registerPushToken(_credentials: AuthCredentials, token: string): Promise<void> {
    const API_ENDPOINT = requireServerUrl();
    await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/push-tokens`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ token })
        });

        if (!response.ok) {
            throw new Error(`Failed to register push token: ${response.status}`);
        }

        const data = await response.json();
        if (!data.success) {
            throw new Error('Failed to register push token');
        }
    });
}
