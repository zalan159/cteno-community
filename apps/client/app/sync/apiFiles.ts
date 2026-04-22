import { AuthCredentials } from '@/auth/tokenStorage';
import { backoff } from '@/utils/time';
import { authedFetch } from './authedFetch';
import { getServerUrl, isServerAvailable, requireServerUrl } from './serverConfig';

type InitiateImageResult = {
    fileId: string;
    bucket: string;
    endpoint: string;
    objectKey: string;
    expiresAt: number;
    uploadUrl?: string;
    contentType?: string;
    sts: {
        accessKeyId: string;
        accessKeySecret: string;
        securityToken: string;
        expiration: string;
    };
};

type DownloadUrlResult = {
    url: string;
    filename: string;
    mime: string;
    size: number;
    expiresAt: number;
};

// Cache signed download URLs (key: fileId, value: { url, expiresAt })
const urlCache = new Map<string, { url: string; expiresAt: number }>();
const URL_CACHE_MARGIN_MS = 10 * 60 * 1000; // Refresh 10 min before expiry

/**
 * Upload a base64 image directly to OSS using a pre-signed PUT URL.
 * Returns the fileId for referencing in messages.
 */
export async function uploadChatImage(
    _credentials: AuthCredentials,
    base64Data: string,
    mediaType: string,
): Promise<string> {
    const API_ENDPOINT = requireServerUrl();
    const ext = mediaType.split('/')[1] || 'jpeg';
    const filename = `chat-image-${Date.now()}.${ext}`;

    // Decode base64 to binary
    const binaryStr = atob(base64Data);
    const bytes = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
        bytes[i] = binaryStr.charCodeAt(i);
    }

    // Step 1: Get signed upload URL from server
    const initResponse = await authedFetch(`${API_ENDPOINT}/v1/files/initiate-image`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ filename, mime: mediaType, size: bytes.length }),
    });

    if (!initResponse.ok) {
        const err = await initResponse.json().catch(() => ({}));
        throw new Error((err as any).error || `Upload initiate failed: ${initResponse.status}`);
    }

    const initResult = await initResponse.json() as InitiateImageResult;

    if (!initResult.uploadUrl) {
        throw new Error('Server did not return uploadUrl');
    }

    // Step 2: PUT directly to OSS (no traffic through our server)
    const ossResponse = await fetch(initResult.uploadUrl, {
        method: 'PUT',
        headers: {
            'Content-Type': initResult.contentType || mediaType,
        },
        body: bytes,
    });

    if (!ossResponse.ok) {
        const errText = await ossResponse.text();
        throw new Error(`OSS upload failed: ${ossResponse.status} ${errText}`);
    }

    // Step 3: Mark file as uploaded on our server
    await authedFetch(`${API_ENDPOINT}/v1/files/complete`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ fileId: initResult.fileId, size: bytes.length, mime: mediaType }),
    });

    return initResult.fileId;
}

/**
 * Get a signed download URL for a file.
 * Pass thumbnailWidth to get a resized image (OSS image processing).
 * Results are cached for ~50 minutes (URLs valid for 1 hour).
 */
export async function getImageDownloadUrl(
    _credentials: AuthCredentials,
    fileId: string,
    thumbnailWidth?: number,
): Promise<string> {
    const cacheKey = thumbnailWidth ? `${fileId}:t${thumbnailWidth}` : fileId;

    const cached = urlCache.get(cacheKey);
    if (cached && cached.expiresAt > Date.now() + URL_CACHE_MARGIN_MS) {
        return cached.url;
    }

    const API_ENDPOINT = requireServerUrl();
    const query = thumbnailWidth ? `?thumbnail=${thumbnailWidth}` : '';
    const response = await authedFetch(`${API_ENDPOINT}/v1/files/${fileId}/download${query}`);

    if (!response.ok) {
        throw new Error(`Failed to get download URL: ${response.status}`);
    }

    const data = await response.json() as DownloadUrlResult;
    urlCache.set(cacheKey, { url: data.url, expiresAt: data.expiresAt });
    return data.url;
}

export async function getFileDownloadUrl(
    _credentials: AuthCredentials,
    fileId: string,
): Promise<DownloadUrlResult> {
    if (!isServerAvailable()) {
        throw new Error('Server unavailable');
    }
    const API_ENDPOINT = getServerUrl();

    return await backoff(async () => {
        const response = await authedFetch(`${API_ENDPOINT}/v1/files/${fileId}/download`, {
            headers: {
                'Content-Type': 'application/json',
            },
        });

        if (!response.ok) {
            if (response.status === 404) {
                throw new Error('File not found');
            }
            throw new Error(`Failed to get download URL: ${response.status}`);
        }

        return await response.json() as DownloadUrlResult;
    });
}
