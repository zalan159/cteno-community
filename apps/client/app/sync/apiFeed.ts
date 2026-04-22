import { AuthCredentials } from '@/auth/tokenStorage';
import { backoff } from '@/utils/time';
import { authedFetch, NotAuthenticatedError } from './authedFetch';
import { getServerUrl, isServerAvailable } from './serverConfig';
import { FeedResponse, FeedResponseSchema, FeedItem } from './feedTypes';
import { log } from '@/log';

/**
 * Fetch user's feed with pagination
 */
export async function fetchFeed(
    _credentials: AuthCredentials,
    options?: {
        limit?: number;
        before?: string;
        after?: string;
    }
): Promise<{ items: FeedItem[]; hasMore: boolean }> {
    if (!isServerAvailable()) {
        return { items: [], hasMore: false };
    }
    const API_ENDPOINT = getServerUrl();

    return await backoff(async () => {
        const params = new URLSearchParams();
        if (options?.limit) params.set('limit', options.limit.toString());
        if (options?.before) params.set('before', options.before);
        if (options?.after) params.set('after', options.after);

        const url = `${API_ENDPOINT}/v1/feed${params.toString() ? `?${params}` : ''}`;
        log.log(`📰 Fetching feed: ${url}`);

        let response: Response;
        try {
            response = await authedFetch(url, { method: 'GET' });
        } catch (err) {
            if (err instanceof NotAuthenticatedError) {
                return { items: [] as FeedItem[], hasMore: false };
            }
            throw err;
        }

        if (!response.ok) {
            throw new Error(`Failed to fetch feed: ${response.status}`);
        }

        const data = await response.json();
        const parsed = FeedResponseSchema.safeParse(data);

        if (!parsed.success) {
            console.error('Failed to parse feed response:', parsed.error);
            throw new Error('Invalid feed response format');
        }

        // Add counter field from cursor
        const itemsWithCounter: FeedItem[] = parsed.data.items.map(item => ({
            ...item,
            counter: parseInt(item.cursor.substring(2), 10) // Extract counter from cursor format "0-{counter}"
        }));

        return {
            items: itemsWithCounter,
            hasMore: parsed.data.hasMore
        };
    });
}
