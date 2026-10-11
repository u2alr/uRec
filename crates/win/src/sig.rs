//! ECDSA P-256 signature check with Windows' own crypto (CNG), used to verify signed update
//! checksums. No third-party crypto code.

use windows::Win32::Security::Cryptography::{
    BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptImportKeyPair, BCryptOpenAlgorithmProvider,
    BCryptVerifySignature, BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
    BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
};

/// `public_xy`: the 64-byte public point (X || Y), `hash`: SHA-256 of the signed data,
/// `sig`: 64-byte IEEE P1363 signature (r || s).
pub fn verify_p256(public_xy: &[u8; 64], hash: &[u8; 32], sig: &[u8; 64]) -> bool {
    unsafe {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        if BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_ECDSA_P256_ALGORITHM,
            None,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .is_err()
        {
            return false;
        }
        // BCRYPT_ECCKEY_BLOB header (magic, key length) followed by X and Y.
        let mut blob = Vec::with_capacity(8 + 64);
        blob.extend_from_slice(&BCRYPT_ECDSA_PUBLIC_P256_MAGIC.to_le_bytes());
        blob.extend_from_slice(&32u32.to_le_bytes());
        blob.extend_from_slice(public_xy);
        let mut key = BCRYPT_KEY_HANDLE::default();
        let ok = BCryptImportKeyPair(alg, None, BCRYPT_ECCPUBLIC_BLOB, &mut key, &blob, 0).is_ok()
            && BCryptVerifySignature(key, None, hash, sig, BCRYPT_FLAGS(0)).is_ok();
        if !key.is_invalid() {
            let _ = BCryptDestroyKey(key);
        }
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        ok
    }
}
