import * as crypto from 'rn-encryption';
import { decodeUTF8, encodeUTF8 } from './text';
import { decodeBase64, encodeBase64 } from '@/encryption/base64';
import { Platform } from 'react-native';

// Web-safe AES-GCM that avoids String.fromCharCode(...hugeArray) stack overflow
// in web-secure-encryption. Uses Web Crypto API directly on web platform.
async function webEncryptAES(data: string, key64: string): Promise<string> {
    const keyBytes = decodeBase64(key64);
    const keyData = new Uint8Array(keyBytes.length);
    keyData.set(keyBytes);
    const cryptoKey = await globalThis.crypto.subtle.importKey(
        'raw', keyData, { name: 'AES-GCM' }, false, ['encrypt'],
    );
    const iv = globalThis.crypto.getRandomValues(new Uint8Array(12));
    const encoded = new TextEncoder().encode(data);
    const ciphertext = await globalThis.crypto.subtle.encrypt(
        { name: 'AES-GCM', iv }, cryptoKey, encoded,
    );
    const combined = new Uint8Array(iv.length + ciphertext.byteLength);
    combined.set(iv);
    combined.set(new Uint8Array(ciphertext), iv.length);
    return encodeBase64(combined);
}

async function webDecryptAES(data: string, key64: string): Promise<string> {
    const keyBytes = decodeBase64(key64);
    const keyData = new Uint8Array(keyBytes.length);
    keyData.set(keyBytes);
    const cryptoKey = await globalThis.crypto.subtle.importKey(
        'raw', keyData, { name: 'AES-GCM' }, false, ['decrypt'],
    );
    const combined = decodeBase64(data);
    const iv = combined.slice(0, 12);
    const ciphertext = combined.slice(12);
    const decrypted = await globalThis.crypto.subtle.decrypt(
        { name: 'AES-GCM', iv }, cryptoKey, ciphertext,
    );
    return new TextDecoder().decode(decrypted);
}

const isWeb = Platform.OS === 'web';

export async function encryptAESGCMString(data: string, key64: string): Promise<string> {
    if (isWeb) return webEncryptAES(data, key64);
    return await crypto.encryptAsyncAES(data, key64);
}

export async function decryptAESGCMString(data: string, key64: string): Promise<string | null> {
    if (isWeb) return webDecryptAES(data, key64);
    const res = (await crypto.decryptAsyncAES(data, key64)).trim();
    return res;
}

export async function encryptAESGCM(data: Uint8Array, key64: string): Promise<Uint8Array> {
    const encrypted = (await encryptAESGCMString(decodeUTF8(data), key64)).trim();
    return decodeBase64(encrypted);
}
export async function decryptAESGCM(data: Uint8Array, key64: string): Promise<Uint8Array | null> {
    let raw = await decryptAESGCMString(encodeBase64(data), key64);
    return raw ? encodeUTF8(raw) : null;
}
