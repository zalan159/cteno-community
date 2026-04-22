/**
 * Demo Mode - Global state for App Store review demo
 *
 * When active, the app shows a simulated chat session without
 * requiring authentication or a real desktop machine.
 */

let _isDemoMode = false;

export function isDemoMode(): boolean {
    return _isDemoMode;
}

export function setDemoMode(active: boolean) {
    _isDemoMode = active;
}

export const DEMO_SESSION_ID = 'demo-session';
