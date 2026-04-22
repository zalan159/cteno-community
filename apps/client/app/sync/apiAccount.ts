import { AuthCredentials } from '@/auth/tokenStorage';
import { authedFetch } from './authedFetch';
import { getServerUrl, isServerAvailable } from './serverConfig';

/**
 * Request permanent deletion of the authenticated user's account
 * and all associated data on the server.
 */
export async function deleteAccount(_credentials: AuthCredentials): Promise<void> {
    if (!isServerAvailable()) {
        throw new Error('Server unavailable');
    }
    const serverUrl = getServerUrl();

    const response = await authedFetch(`${serverUrl}/v1/account`, {
        method: 'DELETE',
    });

    if (!response.ok) {
        const body = await response.json().catch(() => ({}));
        throw new Error(body.error || `Failed to delete account: ${response.status}`);
    }
}
