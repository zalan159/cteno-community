import AsyncStorage from '@react-native-async-storage/async-storage';
import * as AppleAuthentication from 'expo-apple-authentication';
import { getRandomBytes } from 'expo-crypto';
import { Platform } from 'react-native';

import type { AuthSuccessPayload } from '@/auth/tokenStorage';
import { encodeBase64 } from '@/encryption/base64';
import { getPublicKeyForBox } from '@/encryption/libsodium';
import { requireServerUrl } from '@/sync/serverConfig';

const APPLE_DEVICE_PUBLIC_KEY_STORAGE_KEY = 'auth_apple_device_public_key_v1';
const PUBLIC_KEY_SEED_BYTES = 32;

type AppleAuthSuccessResponse = {
    accessToken: string;
    refreshToken: string;
    expiresIn: number;
    refreshExpiresIn: number;
    userId: string;
};

type AppleAuthErrorResponse = {
    error?: string;
};

async function getOrCreateAppleDevicePublicKey(): Promise<string> {
    const existing = await AsyncStorage.getItem(APPLE_DEVICE_PUBLIC_KEY_STORAGE_KEY);
    if (existing) {
        return existing;
    }

    const publicKey = encodeBase64(getPublicKeyForBox(getRandomBytes(PUBLIC_KEY_SEED_BYTES)));
    await AsyncStorage.setItem(APPLE_DEVICE_PUBLIC_KEY_STORAGE_KEY, publicKey);
    return publicKey;
}

function isAppleSignInCanceled(error: unknown): boolean {
    if (!error || typeof error !== 'object') {
        return false;
    }

    return 'code' in error && (error.code === 'ERR_REQUEST_CANCELED' || error.code === 'ERR_CANCELED');
}

function getAppleAuthErrorMessage(payload: AppleAuthSuccessResponse | AppleAuthErrorResponse | null, status: number): string {
    if (payload && 'error' in payload && typeof payload.error === 'string' && payload.error) {
        return payload.error;
    }

    return `Apple login failed with status ${status}.`;
}

export async function signInWithApple(): Promise<AuthSuccessPayload | null> {
    if (Platform.OS !== 'ios') {
        throw new Error('Apple Sign In is only available on iOS.');
    }

    if (!(await AppleAuthentication.isAvailableAsync())) {
        throw new Error('Apple Sign In is unavailable on this device.');
    }

    try {
        const credential = await AppleAuthentication.signInAsync({
            requestedScopes: [
                AppleAuthentication.AppleAuthenticationScope.FULL_NAME,
                AppleAuthentication.AppleAuthenticationScope.EMAIL,
            ],
        });

        if (!credential.identityToken) {
            throw new Error('Apple Sign In did not return an identity token.');
        }

        const response = await fetch(`${requireServerUrl()}/v1/auth/apple`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                identityToken: credential.identityToken,
                user: credential.user ?? null,
                fullName: credential.fullName
                    ? {
                        givenName: credential.fullName.givenName,
                        familyName: credential.fullName.familyName,
                    }
                    : null,
                publicKey: await getOrCreateAppleDevicePublicKey(),
            }),
        });

        const payload = await response.json().catch(() => null) as AppleAuthSuccessResponse | AppleAuthErrorResponse | null;

        if (!response.ok || !payload || !('accessToken' in payload)) {
            throw new Error(getAppleAuthErrorMessage(payload, response.status));
        }

        return payload;
    } catch (error) {
        if (isAppleSignInCanceled(error)) {
            return null;
        }
        throw error;
    }
}
