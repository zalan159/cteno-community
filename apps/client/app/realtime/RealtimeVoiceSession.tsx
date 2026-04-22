import React, { useEffect, useRef } from 'react';
import { AudioRecorder } from 'react-native-audio-api';
import { randomUUID } from 'expo-crypto';
import { registerSpeechToTextSession, getTranscriptionCallback } from './RealtimeSession';
import { storage } from '@/sync/storage';
import type { SpeechToTextSession, SpeechToTextConfig } from './types';

function generateTaskId(): string {
    return randomUUID().replace(/-/g, '');
}

class NlsSpeechToTextSession implements SpeechToTextSession {
    private ws: WebSocket | null = null;
    private recorder: AudioRecorder | null = null;
    private taskId: string = '';
    private appkey: string = '';
    private _active: boolean = false;

    async start(config: SpeechToTextConfig): Promise<void> {
        this.taskId = generateTaskId();
        this.appkey = config.appkey;

        const wsUrl = `wss://nls-gateway-cn-shanghai.aliyuncs.com/ws/v1?token=${config.token}`;

        return new Promise((resolve, reject) => {
            this.ws = new WebSocket(wsUrl);

            this.ws.onopen = () => {
                const startMsg = {
                    header: {
                        message_id: generateTaskId(),
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

    private startAudioCapture() {
        try {
            this.recorder = new AudioRecorder({
                sampleRate: 16000,
                bufferLengthInSamples: 2048
            });

            this.recorder.onAudioReady((event) => {
                if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
                const inputData = event.buffer.getChannelData(0);
                const len = event.numFrames;
                const pcm16 = new Int16Array(len);
                for (let i = 0; i < len; i++) {
                    pcm16[i] = Math.max(-1, Math.min(1, inputData[i])) * 0x7FFF;
                }
                this.ws.send(pcm16.buffer);
            });

            this.recorder.start();
        } catch (e) {
            console.error('[NLS] Failed to start audio capture:', e);
            storage.getState().setRealtimeStatus('error');
            this.cleanup();
        }
    }

    async stop(): Promise<void> {
        this.stopAudioCapture();

        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            const stopMsg = {
                header: {
                    message_id: generateTaskId(),
                    task_id: this.taskId,
                    namespace: 'SpeechTranscriber',
                    name: 'StopTranscription',
                    appkey: this.appkey
                }
            };
            this.ws.send(JSON.stringify(stopMsg));

            await new Promise<void>((resolve) => {
                const timeout = setTimeout(() => {
                    this.cleanup();
                    resolve();
                }, 2000);

                const origHandler = this.ws!.onmessage;
                this.ws!.onmessage = (event) => {
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
        if (this.recorder) {
            this.recorder.stop();
            this.recorder = null;
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
