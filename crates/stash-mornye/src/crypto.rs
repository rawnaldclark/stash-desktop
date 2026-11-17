//! Ciphers add-ons ask the host for (spec §2.11, §2.12; ported from Lokal's `rust/src/morneye_crypto.rs`, which
//! matches Lokal desktop's Node crypto and egoroof-blowfish).

use aes::{Aes128, Aes192, Aes256};
use aes_gcm::Aes256Gcm;
use aes_gcm::aead::Aead;
use blowfish::Blowfish;
use cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// AES (128/192/256 by the key's length, or the length `algorithm` names) or Blowfish (4 to 56 byte key), CBC or
/// CTR, as `encrypt/decryptBlockCipher` and `transformPatternedBlocks` use them. `padding` is PKCS#7 for CBC only.
#[allow(clippy::too_many_arguments)]
pub fn block_cipher(
    encrypt: bool,
    algorithm: &str,
    mode: &str,
    key: &[u8],
    iv: &[u8],
    pkcs7: bool,
    data: &[u8],
) -> Result<Vec<u8>, String> {
    BlockCipher::new(algorithm, mode, key, iv, pkcs7)?.apply(encrypt, data)
}

/// The block size of `algorithm` (16 for AES, 8 for Blowfish).
pub fn block_size(algorithm: &str) -> Result<usize, String> {
    match algorithm.to_lowercase().as_str() {
        "blowfish" => Ok(8),
        "aes" | "aes-128" | "aes-192" | "aes-256" => Ok(16),
        _ => Err("Unsupported cipher".into()),
    }
}

/// `utils.encrypt`: AES-256-GCM with key SHA-256(UTF-8(key)), a random 12-byte nonce; base64(nonce‖ciphertext‖tag).
pub fn seal_text(text: &str, key: &str) -> Result<String, String> {
    let sealed = seal(&sha256(key.as_bytes()), text.as_bytes())?;
    Ok(crate::bytes::base64(&sealed))
}

/// `utils.decrypt`, the reverse of [`seal_text`].
pub fn open_text(sealed: &str, key: &str) -> Result<String, String> {
    let bytes = crate::bytes::decode(&serde_json::Value::String(sealed.to_string()), "base64")?;
    let plain = open(&sha256(key.as_bytes()), &bytes)?;
    String::from_utf8(plain).map_err(|_| "Decrypted data is not valid UTF-8".to_string())
}

/// AES-256-GCM with `key`: nonce‖ciphertext‖tag (the store's sealing).
pub fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>, String> {
    seal_with_nonce(key, plain, &crate::random_bytes(12))
}

/// The reverse of [`seal`].
pub fn open(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < 28 {
        return Err("Invalid encrypted addon data".into());
    }
    let gcm = <Aes256Gcm as aes_gcm::KeyInit>::new_from_slice(key)
        .map_err(|_| "Invalid key".to_string())?;
    let nonce = aes_gcm::aead::Nonce::<Aes256Gcm>::try_from(&sealed[..12])
        .map_err(|_| "Invalid encrypted addon data".to_string())?;
    gcm.decrypt(&nonce, &sealed[12..])
        .map_err(|_| "Invalid encrypted addon data".to_string())
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = <Sha256 as Digest>::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub fn sha1(bytes: &[u8]) -> [u8; 20] {
    let digest = <::sha1::Sha1 as Digest>::digest(bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&digest);
    out
}

pub fn md5(bytes: &[u8]) -> [u8; 16] {
    let digest = <::md5::Md5 as Digest>::digest(bytes);
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest);
    out
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key)
        .expect("HMAC takes a key of any length");
    Mac::update(&mut mac, message);
    let tag = Mac::finalize(mac).into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&tag);
    out
}

pub fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut mac = <Hmac<::sha1::Sha1> as hmac::KeyInit>::new_from_slice(key)
        .expect("HMAC takes a key of any length");
    Mac::update(&mut mac, message);
    let tag = Mac::finalize(mac).into_bytes();
    let mut out = [0u8; 20];
    out.copy_from_slice(&tag);
    out
}

/// AES-256-GCM with a given nonce: nonce (12) ‖ ciphertext ‖ tag (16).
fn seal_with_nonce(key: &[u8], plain: &[u8], nonce: &[u8]) -> Result<Vec<u8>, String> {
    if nonce.len() != 12 {
        return Err("Invalid nonce".into());
    }
    let gcm = <Aes256Gcm as aes_gcm::KeyInit>::new_from_slice(key)
        .map_err(|_| "Invalid key".to_string())?;
    let nonce = aes_gcm::aead::Nonce::<Aes256Gcm>::try_from(nonce)
        .map_err(|_| "Invalid nonce".to_string())?;
    let sealed = gcm
        .encrypt(&nonce, plain)
        .map_err(|_| "Encryption failed".to_string())?;
    Ok([&nonce[..], &sealed[..]].concat())
}

/// One block cipher keyed for both directions (one at a time, for one call: Blowfish's size does not matter).
#[allow(clippy::large_enum_variant)]
enum Keyed {
    Aes128(Aes128),
    Aes192(Aes192),
    Aes256(Aes256),
    Blowfish(Blowfish),
}

impl Keyed {
    fn new(algorithm: &str, key: &[u8]) -> Result<Keyed, String> {
        let algorithm = algorithm.to_lowercase();
        if matches!(
            algorithm.as_str(),
            "aes" | "aes-128" | "aes-192" | "aes-256"
        ) {
            // `aes-NNN` must name the key's own length.
            if algorithm != "aes" && algorithm[4..].parse::<usize>().ok() != Some(key.len() * 8) {
                return Err("Invalid AES key length".into());
            }
            let keyed = match key.len() {
                16 => <Aes128 as cipher::KeyInit>::new_from_slice(key).map(Keyed::Aes128),
                24 => <Aes192 as cipher::KeyInit>::new_from_slice(key).map(Keyed::Aes192),
                32 => <Aes256 as cipher::KeyInit>::new_from_slice(key).map(Keyed::Aes256),
                _ => return Err("Invalid AES key length".into()),
            };
            return keyed.map_err(|_| "Invalid AES key length".to_string());
        }
        if algorithm != "blowfish" || key.len() < 4 || key.len() > 56 {
            return Err("Unsupported cipher or invalid key length".into());
        }
        <Blowfish as cipher::KeyInit>::new_from_slice(key)
            .map(Keyed::Blowfish)
            .map_err(|_| "Invalid Blowfish key length".to_string())
    }

    fn encrypt(&self, block: &mut [u8]) {
        match self {
            Keyed::Aes128(c) => {
                c.encrypt_block(<&mut Block<Aes128>>::try_from(block).expect("a whole block"))
            }
            Keyed::Aes192(c) => {
                c.encrypt_block(<&mut Block<Aes192>>::try_from(block).expect("a whole block"))
            }
            Keyed::Aes256(c) => {
                c.encrypt_block(<&mut Block<Aes256>>::try_from(block).expect("a whole block"))
            }
            Keyed::Blowfish(c) => {
                c.encrypt_block(<&mut Block<Blowfish>>::try_from(block).expect("a whole block"))
            }
        }
    }

    fn decrypt(&self, block: &mut [u8]) {
        match self {
            Keyed::Aes128(c) => {
                c.decrypt_block(<&mut Block<Aes128>>::try_from(block).expect("a whole block"))
            }
            Keyed::Aes192(c) => {
                c.decrypt_block(<&mut Block<Aes192>>::try_from(block).expect("a whole block"))
            }
            Keyed::Aes256(c) => {
                c.decrypt_block(<&mut Block<Aes256>>::try_from(block).expect("a whole block"))
            }
            Keyed::Blowfish(c) => {
                c.decrypt_block(<&mut Block<Blowfish>>::try_from(block).expect("a whole block"))
            }
        }
    }
}

/// A cipher keyed once and applied to many pieces, each from the same IV (a patterned transform's blocks).
pub(crate) struct BlockCipher {
    keyed: Keyed,
    size: usize,
    ctr: bool,
    iv: Vec<u8>,
    pkcs7: bool,
}

impl BlockCipher {
    /// Checks, in Lokal's order: the mode, the IV's length, then the algorithm and key.
    pub(crate) fn new(
        algorithm: &str,
        mode: &str,
        key: &[u8],
        iv: &[u8],
        pkcs7: bool,
    ) -> Result<Self, String> {
        let mode = mode.to_lowercase();
        if mode != "cbc" && mode != "ctr" {
            return Err("Unsupported block cipher mode".into());
        }
        let size = if algorithm.eq_ignore_ascii_case("blowfish") {
            8
        } else {
            16
        };
        if iv.len() != size {
            return Err(format!("IV must be {size} bytes"));
        }
        Ok(BlockCipher {
            keyed: Keyed::new(algorithm, key)?,
            size,
            ctr: mode == "ctr",
            iv: iv.to_vec(),
            pkcs7,
        })
    }

    /// CBC pads with PKCS#7 only when asked (Node's `setAutoPadding`); CTR counts the whole IV as one big-endian
    /// number, and takes a short last block.
    pub(crate) fn apply(&self, encrypt: bool, input: &[u8]) -> Result<Vec<u8>, String> {
        let size = self.size;
        let mut chain = self.iv.clone();
        if self.ctr {
            let mut out = input.to_vec();
            for chunk in out.chunks_mut(size) {
                let mut pad = chain.clone();
                self.keyed.encrypt(&mut pad);
                for (byte, mask) in chunk.iter_mut().zip(pad) {
                    *byte ^= mask;
                }
                for byte in chain.iter_mut().rev() {
                    *byte = byte.wrapping_add(1);
                    if *byte != 0 {
                        break;
                    }
                }
            }
            return Ok(out);
        }
        let mut data = input.to_vec();
        if encrypt && self.pkcs7 {
            let count = size - data.len() % size;
            data.extend(std::iter::repeat_n(count as u8, count));
        }
        if !data.len().is_multiple_of(size) {
            return Err("CBC payload must be block aligned".into());
        }
        for block in data.chunks_mut(size) {
            if encrypt {
                for (byte, previous) in block.iter_mut().zip(&chain) {
                    *byte ^= previous;
                }
                self.keyed.encrypt(block);
                chain.copy_from_slice(block);
            } else {
                let saved = block.to_vec();
                self.keyed.decrypt(block);
                for (byte, previous) in block.iter_mut().zip(&chain) {
                    *byte ^= previous;
                }
                chain = saved;
            }
        }
        if !encrypt && self.pkcs7 {
            let count = *data.last().ok_or("Invalid cipher padding")? as usize;
            if count == 0
                || count > size
                || count > data.len()
                || !data[data.len() - count..]
                    .iter()
                    .all(|&b| b as usize == count)
            {
                return Err("Invalid cipher padding".into());
            }
            data.truncate(data.len() - count);
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    /// The 128-bit value NIST SP 800-38A's examples use (public).
    const NIST_AES_128: &str = "2b7e151628aed2a6abf7158809cf4f3c";

    #[test]
    fn aes_cbc_matches_nist() {
        // NIST SP 800-38A F.2.1, first block.
        let key = unhex(NIST_AES_128);
        let iv = unhex("000102030405060708090a0b0c0d0e0f");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172a");
        let out = block_cipher(true, "aes", "cbc", &key, &iv, false, &plain).unwrap();
        assert_eq!(out, unhex("7649abac8119b246cee98e9b12e9197d"));
        assert_eq!(
            block_cipher(false, "aes-128", "cbc", &key, &iv, false, &out).unwrap(),
            plain
        );
    }

    #[test]
    fn aes_ctr_matches_nist_and_takes_a_short_block() {
        // NIST SP 800-38A F.5.1.
        let key = unhex(NIST_AES_128);
        let iv = unhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let out = block_cipher(true, "aes", "CTR", &key, &iv, false, &plain).unwrap();
        assert_eq!(
            out,
            unhex("874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff")
        );
        assert_eq!(
            block_cipher(false, "aes", "ctr", &key, &iv, false, &out[..20]).unwrap(),
            &plain[..20]
        );
    }

    #[test]
    fn aes_256_cbc_matches_nist() {
        // NIST SP 800-38A F.2.5, first block.
        let key = unhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
        let iv = unhex("000102030405060708090a0b0c0d0e0f");
        let plain = unhex("6bc1bee22e409f96e93d7e117393172a");
        let out = block_cipher(true, "aes-256", "cbc", &key, &iv, false, &plain).unwrap();
        assert_eq!(out, unhex("f58c4c04d6e5f1ba779eabfb5f7bfbd6"));
    }

    #[test]
    fn blowfish_matches_schneiers_vector() {
        // Schneier's vector: an all-zero key and block give 4EF997456198DD78 (one block, zero IV).
        let zero = unhex("0000000000000000");
        let out = block_cipher(true, "blowfish", "cbc", &zero, &zero, false, &zero).unwrap();
        assert_eq!(out, unhex("4ef997456198dd78"));
    }

    #[test]
    fn blowfish_cbc_round_trips_with_padding() {
        let key = b"stash-test-key-01".to_vec();
        let iv = unhex("0001020304050607");
        let plain = b"a song in pieces".to_vec();
        let out = block_cipher(true, "blowfish", "cbc", &key, &iv, true, &plain).unwrap();
        assert_eq!(out.len(), 24);
        assert_ne!(&out[..16], &plain[..]);
        assert_eq!(
            block_cipher(false, "Blowfish", "cbc", &key, &iv, true, &out).unwrap(),
            plain
        );
    }

    #[test]
    fn a_keyed_cipher_starts_each_piece_from_the_iv() {
        let cipher =
            BlockCipher::new("blowfish", "cbc", b"0123456789abcdef", &[1; 8], false).unwrap();
        let block = [9u8; 16];
        let once = cipher.apply(true, &block).unwrap();
        assert_eq!(cipher.apply(true, &block).unwrap(), once);
        assert_eq!(cipher.apply(false, &once).unwrap(), block);
    }

    #[test]
    fn rejects_bad_parameters() {
        let err = |r: Result<Vec<u8>, String>| r.unwrap_err();
        assert_eq!(
            err(block_cipher(
                true, "aes", "ecb", &[0; 16], &[0; 16], false, b"x"
            )),
            "Unsupported block cipher mode"
        );
        assert_eq!(
            err(block_cipher(
                true, "aes-256", "ctr", &[0; 16], &[0; 16], false, b"x"
            )),
            "Invalid AES key length"
        );
        assert_eq!(
            err(block_cipher(
                true, "aes", "cbc", &[0; 16], &[0; 16], false, b"abc"
            )),
            "CBC payload must be block aligned"
        );
        assert_eq!(
            err(block_cipher(
                true, "blowfish", "ctr", &[0; 16], &[0; 16], false, b"x"
            )),
            "IV must be 8 bytes"
        );
        assert_eq!(
            err(block_cipher(
                true, "aes", "cbc", &[0; 16], &[0; 8], false, b"x"
            )),
            "IV must be 16 bytes"
        );
        assert_eq!(
            err(block_cipher(
                true, "", "cbc", &[0; 16], &[0; 16], false, b"x"
            )),
            "Unsupported cipher or invalid key length"
        );
        assert_eq!(
            err(block_cipher(
                true, "blowfish", "cbc", &[0; 3], &[0; 8], false, b"x"
            )),
            "Unsupported cipher or invalid key length"
        );
        let key = [0u8; 16];
        // A last byte of 0 is never PKCS#7 padding.
        let sealed = block_cipher(true, "aes", "cbc", &key, &[0; 16], false, &[0; 16]).unwrap();
        assert_eq!(
            err(block_cipher(
                false, "aes", "cbc", &key, &[0; 16], true, &sealed
            )),
            "Invalid cipher padding"
        );
    }

    #[test]
    fn block_sizes() {
        assert_eq!(block_size("AES-128"), Ok(16));
        assert_eq!(block_size("blowfish"), Ok(8));
        assert!(block_size("des").is_err());
    }

    #[test]
    fn seal_round_trips_and_detects_tampering() {
        let key = [7u8; 32];
        let sealed = seal(&key, b"token").unwrap();
        assert_eq!(sealed.len(), 12 + 5 + 16);
        assert_eq!(open(&key, &sealed).unwrap(), b"token");
        let mut broken = sealed.clone();
        broken[14] ^= 1;
        assert!(open(&key, &broken).is_err());
        assert!(open(&[8u8; 32], &sealed).is_err());
        assert!(open(&key, &sealed[..20]).is_err());
        // Each seal takes a fresh nonce.
        assert_ne!(seal(&key, b"token").unwrap(), sealed);
        let fixed = seal_with_nonce(&key, b"token", &[1; 12]).unwrap();
        assert_eq!(&fixed[..12], &[1; 12]);
    }

    #[test]
    fn text_sealing_round_trips() {
        let sealed = seal_text("héllo", "a key").unwrap();
        assert_eq!(open_text(&sealed, "a key").unwrap(), "héllo");
        assert!(open_text(&sealed, "another key").is_err());
        assert!(open_text("not base64!", "a key").is_err());
    }

    #[test]
    fn digests_match_known_values() {
        assert_eq!(
            crate::hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            crate::hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(crate::hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        // RFC 4231 test case 2 and RFC 2202 test case 2.
        assert_eq!(
            crate::hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            crate::hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }
}
