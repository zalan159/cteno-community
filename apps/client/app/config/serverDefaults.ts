import { getOptionalHappyServerUrl } from '@/config/runtime';

const LOCAL_ONLY_PLACEHOLDER_SERVER_URL = '';

export function getDefaultServerUrl(): string {
    const configured = getOptionalHappyServerUrl();
    if (configured) {
        return configured.replace(/\/+$/, '');
    }
    return LOCAL_ONLY_PLACEHOLDER_SERVER_URL;
}
