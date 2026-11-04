// SPDX-License-Identifier: MIT OR Apache-2.0
//! OpenPGP backend for Org Crypt (stage 2): symmetric encryption plus the
//! decrypt-reuse cache. Pure Rust via the `pgp` (rPGP) crate.
//!
//! Wire choices, all readable by GnuPG (old and new), matching what EasyPG
//! writes for symmetric encryption:
//! - ASCII armor (`-----BEGIN/END PGP MESSAGE-----`).
//! - AES-256 in a v1 integrity-protected container (SEIPDv1, so GnuPG
//!   versions without AEAD/SEIPDv2 still read it).
//! - Iterated-and-salted S2K with SHA-512.
//!
//! `Recipients` (public-key) encryption is not implemented yet; resolving
//! which key an entry wants already works (`org_crypt_key_for_heading`).
//!
//! The reuse cache mirrors `org-crypt.el`'s text properties: after
//! decrypting, the plaintext hash, key and armor are remembered, so saving
//! an unchanged entry reuses its armor instead of re-encrypting (and instead
//! of asking for the passphrase again). Entries are keyed by heading line,
//! but reuse additionally requires the plaintext hash and key to match, so
//! a stale line number can only cause re-encryption, never wrong output.

use super::CryptKey;
use pgp::composed::{ArmorOptions, Message, MessageBuilder};
use pgp::crypto::{hash::HashAlgorithm, sym::SymmetricKeyAlgorithm};
use pgp::types::{Password, StringToKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Encrypt `plaintext` under `passphrase`; returns the armored message
/// (no trailing newline, so it splices cleanly into entry bodies).
pub fn pgp_encrypt_symmetric(plaintext: &[u8], passphrase: &str) -> Result<String, String> {
    let mut rng = rand::thread_rng();
    let s2k = StringToKey::new_iterated(&mut rng, HashAlgorithm::Sha512, 224);
    let mut builder = MessageBuilder::from_reader("", plaintext)
        .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
    builder
        .encrypt_with_password(s2k, &Password::from(passphrase))
        .map_err(|e| e.to_string())?;
    builder
        .to_armored_string(&mut rng, ArmorOptions::default())
        .map(|a| a.trim_end().to_string())
        .map_err(|e| e.to_string())
}

/// Decrypt an armored message under `passphrase`; returns the raw bytes.
/// The caller checks UTF-8 (entry bodies are text).
pub fn pgp_decrypt_symmetric(armor: &str, passphrase: &str) -> Result<Vec<u8>, String> {
    let (message, _) = Message::from_armor(armor.as_bytes()).map_err(|e| e.to_string())?;
    let mut decrypted = message
        .decrypt_with_password(&Password::from(passphrase))
        .map_err(|e| e.to_string())?;
    // GnuPG compresses by default; EasyPG output usually wraps the literal
    // data in a compressed packet. Peel every layer (normally zero or one).
    while decrypted.is_compressed() {
        decrypted = decrypted.decompress().map_err(|e| e.to_string())?;
    }
    decrypted.as_data_vec().map_err(|e| e.to_string())
}

fn plaintext_hash(plaintext: &str) -> [u8; 32] {
    Sha256::digest(plaintext.as_bytes()).into()
}

#[derive(Debug, Clone)]
struct CachedEntry {
    plaintext_hash: [u8; 32],
    key: CryptKey,
    armor: String,
}

/// Per-buffer decrypt cache for `org-crypt.el`'s reuse behavior.
#[derive(Debug, Default, Clone)]
pub struct CryptCache {
    entries: HashMap<usize, CachedEntry>,
}

impl CryptCache {
    /// Remember a decrypt (or fresh encrypt) for heading `h`.
    pub fn remember(&mut self, heading: usize, plaintext: &str, key: CryptKey, armor: String) {
        self.entries.insert(
            heading,
            CachedEntry {
                plaintext_hash: plaintext_hash(plaintext),
                key,
                armor,
            },
        );
    }

    /// The cached armor for heading `h` when its body still hashes to the
    /// remembered plaintext encrypted for the same key. None otherwise.
    pub fn reuse(&self, heading: usize, plaintext: &str, key: &CryptKey) -> Option<&str> {
        self.entries
            .get(&heading)
            .filter(|e| e.plaintext_hash == plaintext_hash(plaintext) && &e.key == key)
            .map(|e| e.armor.as_str())
    }

    /// Drop every heading not in `headings` (call after saving).
    pub fn prune(&mut self, headings: &[usize]) {
        self.entries.retain(|h, _| headings.contains(h));
    }

    /// Whether anything is remembered (the buffer shows decrypted text).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop everything (revert, mode change).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &str = "Secret body\nwith * stars\n";
    const PASSPHRASE: &str = "correct-horse-battery-staple";

    /// `gpg --symmetric --s2k-mode 3 --s2k-digest-algo SHA512 --cipher-algo AES256`
    /// (GnuPG 2.5.24) of PLAINTEXT under PASSPHRASE.
    const GPG_ITERATED_VECTOR: &str = "-----BEGIN PGP MESSAGE-----\n\njA0ECQMKWW0EZ8t6L5Dq0k4B7RUU07sxjoKmZ3n2q6p3SjYeOLvxwNnXybD6PPWh\nMS1zs2LRfT763CRflRdWbm/QL5gzv0jMuTms/WlRcbT3Up02iHTbhhAbSR4a1Gw=\n=xKos\n-----END PGP MESSAGE-----";

    /// Default `gpg --symmetric` (GnuPG 2.5.24) of the same text and passphrase.
    const GPG_DEFAULT_VECTOR: &str = "-----BEGIN PGP MESSAGE-----\n\njA0ECQMIxIpkXbsy3bnq0k4BZnck20fNsmJ8+oykeD5OKqBj+gkDvmxflJz3wRKI\nTLhH9GVqzo7SqCRR6Bg2kkX+AcbOx1Tw6gK09PIyo2rrJE9FLS0SWFgcj0GAizM=\n=w0cd\n-----END PGP MESSAGE-----";

    #[test]
    fn symmetric_roundtrip() {
        let armor = pgp_encrypt_symmetric(PLAINTEXT.as_bytes(), PASSPHRASE).unwrap();
        assert!(armor.contains("-----BEGIN PGP MESSAGE-----"));
        assert!(armor.contains("-----END PGP MESSAGE-----"));
        let back = pgp_decrypt_symmetric(&armor, PASSPHRASE).unwrap();
        assert_eq!(back, PLAINTEXT.as_bytes());
    }

    #[test]
    fn decrypts_gnupg_iterated_vector() {
        let back = pgp_decrypt_symmetric(GPG_ITERATED_VECTOR, PASSPHRASE).unwrap();
        assert_eq!(back, PLAINTEXT.as_bytes());
    }

    #[test]
    fn decrypts_gnupg_default_vector() {
        let back = pgp_decrypt_symmetric(GPG_DEFAULT_VECTOR, PASSPHRASE).unwrap();
        assert_eq!(back, PLAINTEXT.as_bytes());
    }

    #[test]
    fn wrong_passphrase_fails() {
        let armor = pgp_encrypt_symmetric(PLAINTEXT.as_bytes(), PASSPHRASE).unwrap();
        assert!(pgp_decrypt_symmetric(&armor, "wrong").is_err());
        assert!(pgp_decrypt_symmetric(GPG_ITERATED_VECTOR, "wrong").is_err());
    }

    #[test]
    fn garbage_fails() {
        assert!(pgp_decrypt_symmetric("not armor", PASSPHRASE).is_err());
        assert!(pgp_decrypt_symmetric("", PASSPHRASE).is_err());
    }

    #[test]
    fn cache_reuse_needs_same_text_and_key() {
        let mut c = CryptCache::default();
        let key = CryptKey::Symmetric;
        assert_eq!(c.reuse(0, PLAINTEXT, &key), None);
        c.remember(0, PLAINTEXT, key.clone(), "ARMOR".into());
        assert_eq!(c.reuse(0, PLAINTEXT, &key), Some("ARMOR"));
        assert_eq!(c.reuse(0, "edited\n", &key), None);
        assert_eq!(
            c.reuse(0, PLAINTEXT, &CryptKey::Recipients("0xA".into())),
            None
        );
        assert_eq!(c.reuse(1, PLAINTEXT, &key), None);
        c.prune(&[1]);
        assert_eq!(c.len(), 0);
    }
}
