import { Linking } from 'react-native';

/**
 * Open a URL in the external browser.
 * In Tauri, uses shell plugin (window.open is silently ignored in WKWebView).
 * On other platforms, falls back to Linking.openURL.
 */
export async function openExternalUrl(url: string): Promise<void> {
    if (typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window) {
        const { open } = await import('@tauri-apps/plugin-shell');
        await open(url);
    } else {
        await Linking.openURL(url);
    }
}
