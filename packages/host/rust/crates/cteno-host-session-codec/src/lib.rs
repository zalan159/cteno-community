//! Session payload codec and encryption primitives shared by community and
//! commercial runtimes.
//!
//! Local-only sessions use [`SessionMessageCodec::Plaintext`]. Commercial
//! sync/relay can opt into encrypted payloads with the same primitives without
//! forcing community builds to depend on Happy Server client crates.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use crypto_box::{
    aead::AeadCore as BoxAeadCore, Nonce as BoxNonce, PublicKey as BoxPublicKey, SalsaBox,
    SecretKey as BoxSecretKey,
};
use rand::Rng;
use rand::RngCore;
use serde_json::Value;
use xsalsa20poly1305::{Nonce as SecretNonce, XSalsa20Poly1305};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionVariant {
    Legacy,
    DataKey,
}

#[derive(Clone, Copy, Debug)]
pub enum SessionMessageCodec {
    Plaintext,
    Encrypted {
        encryption_key: [u8; 32],
        encryption_variant: EncryptionVariant,
    },
}

impl SessionMessageCodec {
    pub const fn plaintext() -> Self {
        Self::Plaintext
    }

    pub fn for_session_messages(
        encryption_key: [u8; 32],
        encryption_variant: EncryptionVariant,
    ) -> Self {
        if encryption_key == [0; 32] {
            Self::Plaintext
        } else {
            Self::encrypted(encryption_key, encryption_variant)
        }
    }

    pub const fn encrypted(
        encryption_key: [u8; 32],
        encryption_variant: EncryptionVariant,
    ) -> Self {
        Self::Encrypted {
            encryption_key,
            encryption_variant,
        }
    }

    pub fn encode_payload(&self, payload: &[u8]) -> Result<String, String> {
        match self {
            Self::Plaintext => String::from_utf8(payload.to_vec())
                .map_err(|e| format!("Failed to encode plaintext session payload: {}", e)),
            Self::Encrypted {
                encryption_key,
                encryption_variant,
            } => {
                let encrypted = encrypt_data(payload, encryption_key, *encryption_variant)
                    .map_err(|e| format!("Failed to encrypt session payload: {}", e))?;
                Ok(BASE64.encode(encrypted))
            }
        }
    }

    pub fn decode_payload(&self, content_type: &str, content: &str) -> Result<Value, String> {
        match content_type {
            "plaintext" => serde_json::from_str(content)
                .map_err(|e| format!("Plaintext JSON parse failed: {}", e)),
            "encrypted" => match self {
                // Zero-key sessions use the plaintext codec even if a caller
                // still routes through the legacy "encrypted" branch.
                Self::Plaintext => serde_json::from_str(content)
                    .map_err(|e| format!("Plaintext JSON parse failed: {}", e)),
                Self::Encrypted { encryption_key, .. } => {
                    let encrypted_bytes = BASE64
                        .decode(content)
                        .map_err(|e| format!("base64 decode failed: {}", e))?;
                    let decrypted = decrypt_data(&encrypted_bytes, encryption_key)
                        .map_err(|e| format!("decrypt failed: {}", e))?;
                    serde_json::from_slice(&decrypted)
                        .map_err(|e| format!("JSON parse failed: {}", e))
                }
            },
            other => Err(format!("Unsupported session content type: {}", other)),
        }
    }

    pub fn decode_message_content(
        &self,
        content_type: &str,
        content: &Value,
    ) -> Result<Value, String> {
        match content_type {
            "plaintext" => match content {
                Value::String(content) => self.decode_payload(content_type, content),
                value => Ok(value.clone()),
            },
            "encrypted" => {
                let content = content.as_str().ok_or_else(|| {
                    "Encrypted session payload must be a base64 string".to_string()
                })?;
                self.decode_payload(content_type, content)
            }
            other => Err(format!("Unsupported session content type: {}", other)),
        }
    }

    pub fn decode_metadata_blob(&self, encoded_metadata: &str) -> Result<Value, String> {
        let Self::Encrypted { encryption_key, .. } = self else {
            return Err("Encrypted session metadata requires encryption context".to_string());
        };
        let encrypted = BASE64
            .decode(encoded_metadata)
            .map_err(|e| format!("Failed to decode session metadata: {}", e))?;
        let decrypted = decrypt_data(&encrypted, encryption_key)
            .map_err(|e| format!("Failed to decrypt session metadata: {}", e))?;
        serde_json::from_slice(&decrypted)
            .map_err(|e| format!("Failed to parse session metadata: {}", e))
    }
}

pub fn encrypt_data_key(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill(&mut nonce_bytes);
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(1 + 12 + ciphertext.len());
    result.push(0u8);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt_data_key(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if data.len() < 13 {
        return Err("Data too short (missing version/nonce)".to_string());
    }
    if data[0] != 0 {
        return Err("Unsupported data key version".to_string());
    }

    let cipher = Aes256Gcm::new(key.into());
    let nonce = Nonce::from_slice(&data[1..13]);
    cipher
        .decrypt(nonce, &data[13..])
        .map_err(|e| format!("Decryption failed: {}", e))
}

pub fn encrypt_json<T: serde::Serialize>(data: &T, key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let json =
        serde_json::to_string(data).map_err(|e| format!("JSON serialization failed: {}", e))?;
    encrypt_data_key(json.as_bytes(), key)
}

pub fn decrypt_json<T: serde::de::DeserializeOwned>(
    data: &[u8],
    key: &[u8; 32],
) -> Result<T, String> {
    let plaintext = decrypt_data_key(data, key)?;
    serde_json::from_slice(&plaintext).map_err(|e| format!("JSON deserialization failed: {}", e))
}

pub fn encrypt_data(
    data: &[u8],
    key: &[u8; 32],
    variant: EncryptionVariant,
) -> Result<Vec<u8>, String> {
    match variant {
        EncryptionVariant::DataKey => encrypt_data_key(data, key),
        EncryptionVariant::Legacy => encrypt_legacy(data, key),
    }
}

pub fn decrypt_data(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if let Ok(plaintext) = decrypt_data_key(data, key) {
        return Ok(plaintext);
    }
    decrypt_legacy(data, key).map_err(|_| "Decryption failed (no variant matched)".to_string())
}

pub fn encrypt_legacy(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = XSalsa20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = SecretNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| format!("Legacy encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(24 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt_legacy(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if data.len() < 24 {
        return Err("Data too short (missing nonce)".to_string());
    }
    let cipher = XSalsa20Poly1305::new(key.into());
    let nonce = SecretNonce::from_slice(&data[0..24]);
    cipher
        .decrypt(nonce, &data[24..])
        .map_err(|e| format!("Legacy decryption failed: {}", e))
}

pub fn encrypt_box_for_public_key(
    data: &[u8],
    recipient_public_key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let recipient = BoxPublicKey::from(*recipient_public_key);
    let ephemeral_secret = BoxSecretKey::generate(&mut rand::rngs::OsRng);
    let ephemeral_public = ephemeral_secret.public_key();
    let cipher = SalsaBox::new(&recipient, &ephemeral_secret);
    let nonce = SalsaBox::generate_nonce(&mut rand::rngs::OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|e| format!("Box encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(32 + 24 + ciphertext.len());
    result.extend_from_slice(ephemeral_public.as_bytes());
    result.extend_from_slice(nonce.as_slice());
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

pub fn decrypt_box_from_bundle(
    bundle: &[u8],
    recipient_secret_key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if bundle.len() < 56 {
        return Err("Box bundle too short".to_string());
    }
    let ephem_public_bytes: [u8; 32] = bundle[0..32]
        .try_into()
        .map_err(|_| "Invalid ephemeral public key length".to_string())?;
    let ephem_public = BoxPublicKey::from(ephem_public_bytes);
    let nonce = BoxNonce::from_slice(&bundle[32..56]);
    let ciphertext = &bundle[56..];

    let recipient_secret = BoxSecretKey::from(*recipient_secret_key);
    let cipher = SalsaBox::new(&ephem_public, &recipient_secret);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Box decryption failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_key_session_messages_use_plaintext_codec() {
        let codec = SessionMessageCodec::for_session_messages([0; 32], EncryptionVariant::DataKey);
        assert!(matches!(codec, SessionMessageCodec::Plaintext));
    }

    #[test]
    fn plaintext_message_content_accepts_raw_json_objects() {
        let codec = SessionMessageCodec::plaintext();
        let payload = codec
            .decode_message_content("plaintext", &serde_json::json!({ "role": "user" }))
            .expect("plaintext JSON object should decode");

        assert_eq!(payload["role"], "user");
    }

    #[test]
    fn plaintext_codec_accepts_legacy_encrypted_branch_as_raw_json() {
        let codec = SessionMessageCodec::plaintext();
        let payload = codec
            .decode_payload("encrypted", r#"{"role":"user"}"#)
            .expect("zero-key sessions should accept raw JSON on encrypted branch");

        assert_eq!(payload["role"], "user");
    }
}
