function getOptionalEnv(name: string): string | null {
    const value = process.env[name];
    if (!value || !value.trim()) {
        return null;
    }
    return value.trim();
}

export function getOptionalHappyServerUrl(): string | null {
    return getOptionalEnv('EXPO_PUBLIC_HAPPY_SERVER_URL');
}
