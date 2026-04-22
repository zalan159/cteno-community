import React, { useEffect, useRef } from 'react';
import { registerSpeechToTextSession, getTranscriptionCallback } from './RealtimeSession';
import { storage } from '@/sync/storage';
import type { SpeechToTextSession, SpeechToTextConfig } from './types';

function generateUUID(): string {
    return (([1e7] as any) + -1e3 + -4e3 + -8e3 + -1e11).replace(
        /[018]/g,
        (c: number) => (c ^ (crypto.getRandomValues(new Uint8Array(1))[0] & (15 >> (c / 4)))).toString(16)
    ).replace(/-/g, '');
}

class NlsSpeechToTextSession implements SpeechToTextSession {
    private ws: WebSocket | null = null;
    private audioContext: AudioContext | null = null;
    private audioInput: MediaStreamAudioSourceNode | null = null;
    private scriptProcessor: ScriptProcessorNode | null = null;
    private audioStream: MediaStream | null = null;
    private taskId: string = '';
    private appkey: string = '';
    private _active: boolean = false;

    async start(config: SpeechToTextConfig): Promise<void> {
        this.taskId = generateUUID();
        this.appkey = config.appkey;

        const wsUrl = `wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token=${config.token}`;

        return new Promise((resolve, reject) => {
            this.ws = new WebSocket(wsUrl);

            this.ws.onopen = () => {
                // Send StartTranscription command
                const startMsg = {
                    header: {
                        message_id: generateUUID(),
                        task_id: this.taskId,
                        namespace: 'SpeechTranscriber',
                        name: 'StartTranscription',
                        appkey: config.appkey
                    },
                    payload: {
                        format: 'pcm',
                        sample_rate: 16000,
                        enable_intermediate_result: true,
                        enable_punctuation_prediction: true,
                        enable_inverse_text_normalization: true
                    }
                };
                this.ws!.send(JSON.stringify(startMsg));
            };

            this.ws.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data);
                    const name = data.header?.name;
                    const status = data.header?.status;

                    if (status && status !== 20000000) {
                        console.error('[NLS] Error status:', status, data.header?.status_message);
                        storage.getState().setRealtimeStatus('error');
                        this.cleanup();
                        return;
                    }

                    if (name === 'TranscriptionStarted') {
                        console.log('[NLS] Transcription started');
                        this.startAudioCapture();
                        this._active = true;
                        storage.getState().setRealtimeStatus('connected');
                        storage.getState().setRealtimeMode('speaking');
                        resolve();
                    } else if (name === 'TranscriptionResultChanged') {
                        const text = data.payload?.result;
                        const callback = getTranscriptionCallback();
                        if (text && callback) {
                            callback(text, false);
                        }
                    } else if (name === 'SentenceEnd') {
                        const text = data.payload?.result;
                        const callback = getTranscriptionCallback();
                        if (text && callback) {
                            callback(text, true);
                        }
                    } else if (name === 'TranscriptionCompleted') {
                        console.log('[NLS] Transcription completed');
                        this.cleanup();
                    }
                } catch (e) {
                    console.error('[NLS] Failed to parse message:', e);
                }
            };

            this.ws.onerror = (error) => {
                console.error('[NLS] WebSocket error:', error);
                storage.getState().setRealtimeStatus('error');
                this.cleanup();
                reject(error);
            };

            this.ws.onclose = () => {
                console.log('[NLS] WebSocket closed');
                this.cleanup();
            };
        });
    }

    private async startAudioCapture() {
        try {
            this.audioStream = await navigator.mediaDevices.getUserMedia({ audio: true });
            this.audioContext = new AudioContext({ sampleRate: 16000 });
            this.audioInput = this.audioContext.createMediaStreamSource(this.audioStream);
            this.scriptProcessor = this.audioContext.createScriptProcessor(2048, 1, 1);

            this.scriptProcessor.onaudioprocess = (event) => {
                if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
                const inputData = event.inputBuffer.getChannelData(0);
                const pcm16 = new Int16Array(inputData.length);
                for (let i = 0; i < inputData.length; i++) {
                    pcm16[i] = Math.max(-1, Math.min(1, inputData[i])) * 0x7FFF;
                }
                this.ws.send(pcm16.buffer);
            };

            this.audioInput.connect(this.scriptProcessor);
            this.scriptProcessor.connect(this.audioContext.destination);
        } catch (e) {
            console.error('[NLS] Failed to start audio capture:', e);
            storage.getState().setRealtimeStatus('error');
            this.cleanup();
        }
    }

    async stop(): Promise<void> {
        // Stop audio capture first
        this.stopAudioCapture();

        // Send StopTranscription command
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            const stopMsg = {
                header: {
                    message_id: generateUUID(),
                    task_id: this.taskId,
                    namespace: 'SpeechTranscriber',
                    name: 'StopTranscription',
                    appkey: this.appkey
                }
            };
            this.ws.send(JSON.stringify(stopMsg));

            // Wait briefly for TranscriptionCompleted, then close
            await new Promise<void>((resolve) => {
                const timeout = setTimeout(() => {
                    this.cleanup();
                    resolve();
                }, 2000);

                const origHandler = this.ws!.onmessage;
                this.ws!.onmessage = (event) => {
                    // Call original handler for any final results
                    if (origHandler) {
                        (origHandler as any).call(this.ws, event);
                    }
                    try {
                        const data = JSON.parse(event.data);
                        if (data.header?.name === 'TranscriptionCompleted') {
                            clearTimeout(timeout);
                            this.cleanup();
                            resolve();
                        }
                    } catch { /* ignore */ }
                };
            });
        } else {
            this.cleanup();
        }
    }

    private stopAudioCapture() {
        if (this.scriptProcessor) {
            this.scriptProcessor.disconnect();
            this.scriptProcessor = null;
        }
        if (this.audioInput) {
            this.audioInput.disconnect();
            this.audioInput = null;
        }
        if (this.audioStream) {
            this.audioStream.getTracks().forEach(track => track.stop());
            this.audioStream = null;
        }
        if (this.audioContext) {
            this.audioContext.close().catch(() => {});
            this.audioContext = null;
        }
    }

    private cleanup() {
        this.stopAudioCapture();
        if (this.ws) {
            if (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING) {
                this.ws.close();
            }
            this.ws = null;
        }
        this._active = false;
        storage.getState().setRealtimeMode('idle');
    }

    isActive(): boolean {
        return this._active;
    }
}

export const RealtimeVoiceSession: React.FC = () => {
    const hasRegistered = useRef(false);

    useEffect(() => {
        if (!hasRegistered.current) {
            registerSpeechToTextSession(new NlsSpeechToTextSession());
            hasRegistered.current = true;
        }
    }, []);

    return null;
};
