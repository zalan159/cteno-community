import { Platform } from 'react-native';

/**
 * Tauri-specific runtime detection.
 * Used to apply desktop-only UI tuning without affecting normal web builds.
 */
export function isTauri(): boolean {
    return (
        Platform.OS === 'web' &&
        typeof window !== 'undefined' &&
        // Tauri injects this global on the webview window.
        (window as any).__TAURI_INTERNALS__ != null
    );
}

/**
 * Detect if running on macOS (via navigator.platform).
 * Works in both Tauri webview and regular browser.
 */
export function isMacOS(): boolean {
    return typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform);
}

/**
 * Log a message from frontend JS to Rust tracing output.
 * Falls back to console.log when not running in Tauri.
 */
export function frontendLog(message: string, level: 'info' | 'warn' | 'error' | 'debug' = 'info') {
    if (!isTauri()) {
        console.log(`[frontendLog] ${message}`);
        return;
    }
    import('@tauri-apps/api/core').then(({ invoke }) => {
        invoke('frontend_log', { level, message }).catch(() => {});
    });
}

