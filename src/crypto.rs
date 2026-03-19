/// ChaCha20 stream cipher (RFC 8439) — pure Rust, zero dependencies.
///
/// Used to encrypt the bot's minimal state file at rest.
/// Key is derived from the Open WebUI API key at runtime.

/// ChaCha20 quarter round operation.
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// Generate a 64-byte keystream block for the given key, nonce, and block counter.
fn chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    // "expand 32-byte k" constant
    let mut state: [u32; 16] = [
        0x61707865, 0x3320646e, 0x79622d32, 0x6b206574,
        u32::from_le_bytes([key[0], key[1], key[2], key[3]]),
        u32::from_le_bytes([key[4], key[5], key[6], key[7]]),
        u32::from_le_bytes([key[8], key[9], key[10], key[11]]),
        u32::from_le_bytes([key[12], key[13], key[14], key[15]]),
        u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
        u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
        u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
        u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        counter,
        u32::from_le_bytes([nonce[0], nonce[1], nonce[2], nonce[3]]),
        u32::from_le_bytes([nonce[4], nonce[5], nonce[6], nonce[7]]),
        u32::from_le_bytes([nonce[8], nonce[9], nonce[10], nonce[11]]),
    ];

    let initial = state;

    // 20 rounds (10 column rounds + 10 diagonal rounds)
    for _ in 0..10 {
        // Column rounds
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        // Diagonal rounds
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    // Add initial state
    for i in 0..16 {
        state[i] = state[i].wrapping_add(initial[i]);
    }

    // Serialize to bytes
    let mut output = [0u8; 64];
    for i in 0..16 {
        let bytes = state[i].to_le_bytes();
        output[i * 4..i * 4 + 4].copy_from_slice(&bytes);
    }
    output
}

/// Encrypt or decrypt data using ChaCha20 (symmetric — same operation for both).
fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut counter: u32 = 1; // RFC 8439 starts at 1

    for chunk in data.chunks(64) {
        let keystream = chacha20_block(key, nonce, counter);
        for (i, &byte) in chunk.iter().enumerate() {
            output.push(byte ^ keystream[i]);
        }
        counter += 1;
    }

    output
}

/// Derive a 256-bit key from an arbitrary-length secret using repeated hashing.
/// This is a simple key derivation — not HKDF, but sufficient for our use case
/// where the input (API key JWT) already has high entropy.
fn derive_key(secret: &[u8]) -> [u8; 32] {
    // Simple Davies-Meyer-like construction using ChaCha20 itself as a PRF
    let mut key = [0u8; 32];

    // Initial mixing: fold the secret into 32 bytes
    for (i, &byte) in secret.iter().enumerate() {
        key[i % 32] ^= byte;
        // Rotate to spread entropy
        key[i % 32] = key[i % 32].wrapping_add(byte).rotate_left(3) as u8;
    }

    // Run ChaCha20 on the mixed key to produce a well-distributed key
    let nonce = [0u8; 12];
    let block = chacha20_block(&key, &nonce, 0);
    key.copy_from_slice(&block[..32]);

    key
}

/// Generate a random nonce using system entropy.
fn random_nonce() -> [u8; 12] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut nonce = [0u8; 12];

    // Mix multiple entropy sources
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let mut hasher = DefaultHasher::new();
    time.as_nanos().hash(&mut hasher);
    std::process::id().hash(&mut hasher);

    let h1 = hasher.finish();
    nonce[0..8].copy_from_slice(&h1.to_le_bytes());

    // Second hash with different seed
    let mut hasher2 = DefaultHasher::new();
    (time.as_nanos() ^ 0xdeadbeef).hash(&mut hasher2);
    let h2 = hasher2.finish();
    nonce[8..12].copy_from_slice(&h2.to_le_bytes()[..4]);

    nonce
}

/// Encrypt data. Returns nonce (12 bytes) prepended to ciphertext.
pub fn encrypt(secret: &str, plaintext: &[u8]) -> Vec<u8> {
    let key = derive_key(secret.as_bytes());
    let nonce = random_nonce();
    let ciphertext = chacha20_xor(&key, &nonce, plaintext);

    let mut output = Vec::with_capacity(12 + ciphertext.len());
    output.extend_from_slice(&nonce);
    output.extend_from_slice(&ciphertext);
    output
}

/// Decrypt data. Expects nonce (12 bytes) prepended to ciphertext.
pub fn decrypt(secret: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 12 {
        return Err("Data too short to contain nonce".to_string());
    }

    let key = derive_key(secret.as_bytes());
    let nonce: [u8; 12] = data[..12].try_into().unwrap();
    let ciphertext = &data[12..];

    Ok(chacha20_xor(&key, &nonce, ciphertext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let secret = "my-secret-api-key";
        let plaintext = b"group.abc123=dolphin-mistral\ngroup.xyz789=llama3:8b";

        let encrypted = encrypt(secret, plaintext);
        let decrypted = decrypt(secret, &encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_different_secrets_produce_different_output() {
        let plaintext = b"test data";
        let enc1 = encrypt("secret1", plaintext);
        let enc2 = encrypt("secret2", plaintext);

        // Ciphertexts should differ (different keys, different nonces)
        assert_ne!(enc1, enc2);
    }

    #[test]
    fn test_wrong_secret_produces_garbage() {
        let plaintext = b"sensitive settings";
        let encrypted = encrypt("correct-secret", plaintext);
        let decrypted = decrypt("wrong-secret", &encrypted).unwrap();

        assert_ne!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_too_short() {
        assert!(decrypt("key", &[0u8; 5]).is_err());
    }

    #[test]
    fn test_encrypt_empty() {
        let encrypted = encrypt("key", b"");
        let decrypted = decrypt("key", &encrypted).unwrap();
        assert_eq!(decrypted, b"");
    }

    #[test]
    fn test_encrypt_large_data() {
        // Test multi-block encryption (>64 bytes)
        let plaintext = vec![0x42u8; 256];
        let encrypted = encrypt("key", &plaintext);
        let decrypted = decrypt("key", &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_nonce_is_unique() {
        // Two encryptions of the same data should produce different ciphertexts
        let enc1 = encrypt("key", b"same data");
        let enc2 = encrypt("key", b"same data");
        assert_ne!(enc1[..12], enc2[..12]); // nonces differ
    }

    // RFC 8439 Section 2.3.2 test vector
    #[test]
    fn test_chacha20_block_rfc_vector() {
        let key: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
            0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let nonce: [u8; 12] = [
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a,
            0x00, 0x00, 0x00, 0x00,
        ];
        let block = chacha20_block(&key, &nonce, 1);

        // First 4 bytes of expected output from RFC 8439
        assert_eq!(block[0], 0x10);
        assert_eq!(block[1], 0xf1);
        assert_eq!(block[2], 0xe7);
        assert_eq!(block[3], 0xe4);
    }

    #[test]
    fn test_derive_key_deterministic() {
        let k1 = derive_key(b"same-input");
        let k2 = derive_key(b"same-input");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_derive_key_different_inputs() {
        let k1 = derive_key(b"input-a");
        let k2 = derive_key(b"input-b");
        assert_ne!(k1, k2);
    }
}
