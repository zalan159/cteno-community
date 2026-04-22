import type { SpeechToTextSession, TranscriptionCallback } from './types';
import { fetchNlsToken } from '@/sync/apiVoice';
import { storage } from '@/sync/storage';
import { Modal } from '@/modal';
import { TokenStorage } from '@/auth/tokenStorage';
import { t } from '@/text';
import { requestMicrophonePermission, showMicrophonePermissionDeniedAlert } from '@/utils/microphonePermissions';

let sttSession: SpeechToTextSession | null = null;
let globalTranscriptionCallback: TranscriptionCallback | null = null;
let currentRealtimeSessionId: string | null = null;

export function getCurrentRealtimeSessionId(): string | null {
    return currentRealtimeSessionId;
}

export function setCurrentRealtimeSessionId(sessionId: string | null) {
    currentRealtimeSessionId = sessionId;
}

export function getVoiceSession(): SpeechToTextSession | null {
    return sttSession;
}

export function getTranscriptionCallback(): TranscriptionCallback | null {
    return globalTranscriptionCallback;
}

export async function startSpeechToText(onTranscription: TranscriptionCallback) {
    if (!sttSession) {
        console.warn('No speech-to-text session registered');
        return;
    }

    // Request microphone permission
    const permissionResult = await requestMicrophonePermission();
    if (!permissionResult.granted) {
        showMicrophonePermissionDeniedAlert(permissionResult.canAskAgain);
        return;
    }

    try {
        storage.getState().setRealtimeStatus('connecting');
        globalTranscriptionCallback = onTranscription;

        // Fetch NLS token from server
        const credentials = await TokenStorage.getCredentials();
        if (!credentials) {
            Modal.alert(t('common.error'), t('errors.authenticationFailed'));
            storage.getState().setRealtimeStatus('disconnected');
            return;
        }

        const response = await fetchNlsToken(credentials);
        if (!response.allowed || !response.token || !response.appkey) {
            Modal.alert(t('common.error'), 'Speech service unavailable');
            storage.getState().setRealtimeStatus('disconnected');
            return;
        }

        await sttSession.start({
            token: response.token,
            appkey: response.appkey
        });
    } catch (error) {
        console.error('Failed to start speech-to-text:', error);
        storage.getState().setRealtimeStatus('error');
        globalTranscriptionCallback = null;
        Modal.alert(t('common.error'), t('errors.voiceServiceUnavailable'));
    }
}

export async function stopSpeechToText() {
    if (!sttSession) {
        return;
    }

    try {
        await sttSession.stop();
    } catch (error) {
        console.error('Failed to stop speech-to-text:', error);
    }
    globalTranscriptionCallback = null;
    storage.getState().setRealtimeStatus('disconnected');
    storage.getState().setRealtimeMode('idle');
}

export function registerSpeechToTextSession(session: SpeechToTextSession) {
    sttSession = session;
}

export function isSpeechToTextActive(): boolean {
    return sttSession?.isActive() ?? false;
}
