export interface SpeechToTextConfig {
    token: string;
    appkey: string;
}

export interface SpeechToTextSession {
    start(config: SpeechToTextConfig): Promise<void>;
    stop(): Promise<void>;
    isActive(): boolean;
    sendTextMessage?(text: string): void;
}

/** Callback for receiving transcription results */
export type TranscriptionCallback = (text: string, isFinal: boolean) => void;
