import { AuthCredentials } from '@/auth/tokenStorage';
import { backoff } from '@/utils/time';
import { authedFetch } from './authedFetch';
import { requireServerUrl } from './serverConfig';

/**
 * Connect a service to the user's account
 */
export async function connectService(
    _credentials: AuthCredentials,
    service: string,
    token: any
): Promise<void> {
    const API_ENDPOINT = requireServerUrl();

    return await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/connect/${service}/register`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ token: JSON.stringify(token) })
        });

        if (!response.ok) {
            throw new Error(`Failed to connect ${service}: ${response.status}`);
        }

        const data = await response.json() as { success: true };
        if (!data.success) {
            throw new Error(`Failed to connect ${service} account`);
        }
    });
}

/**
 * Disconnect a connected service from the user's account
 */
export async function disconnectService(_credentials: AuthCredentials, service: string): Promise<void> {
    const API_ENDPOINT = requireServerUrl();

    return await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/connect/${service}`, {
            method: 'DELETE',
        });

        if (!response.ok) {
            if (response.status === 404) {
                const error = await response.json();
                throw new Error(error.error || `${service} account not connected`);
            }
            throw new Error(`Failed to disconnect ${service}: ${response.status}`);
        }

        const data = await response.json() as { success: true };
        if (!data.success) {
            throw new Error(`Failed to disconnect ${service} account`);
        }
    });
}
