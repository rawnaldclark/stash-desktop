//! sync-v1 crypto (stash-player `docs/sync-v1.md` §1, §3.1–3.3), byte for byte as the web player's `crypto.ts` and the app's
//! `SyncCrypto.kt`: strict base64url, HKDF-SHA256, AES-256-GCM, gzip(UTF-8 JSON) plaintexts read strictly, and the envelopes
//! `{ e, n, c }` sealed with the AAD `stash-sync/1|<spaceId>|<epoch>|<place>`. All three pass `sync-crypto-vectors.json`.

use std::io::{Read, Write};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const AAD_PREFIX: &str = "stash-sync/1";
pub const INFO_DATA: &str = "stash-sync v1 data";
pub const NONCE_BYTES: usize = 12;
pub const KEY_BYTES: usize = 32;
/// Most bytes a plaintext may inflate to.
pub const MAX_PLAINTEXT: usize = 64 * 1024 * 1024;

/// Something did not decrypt, inflate or read: the caller treats the envelope as not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoError(pub &'static str);

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for CryptoError {}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// base64url without padding.
pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url read strictly (§1): no padding, no whitespace, no `+` or `/`, no length of 4n+1, no stray low bits, so every byte
/// string has one spelling.
pub fn unb64(s: &str) -> Result<Vec<u8>> {
    if !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(CryptoError("bad base64url"));
    }
    URL_SAFE_NO_PAD.decode(s).map_err(|_| CryptoError("bad base64url"))
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut out = [0u8; 32];
    out.copy_from_slice(hmac::sign(&k, data).as_ref());
    out
}

/// HKDF-SHA256 (RFC 5869). An empty salt is 32 zero bytes, as the RFC says.
pub fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    assert!(length <= 255 * 32, "HKDF can't give that much");
    let zeros = [0u8; 32];
    let prk = hmac_sha256(if salt.is_empty() { &zeros } else { salt }, ikm);
    let mut out = Vec::with_capacity(length);
    let mut t: Vec<u8> = Vec::new();
    let mut i = 1u8;
    while out.len() < length {
        let mut block = t.clone();
        block.extend_from_slice(info);
        block.push(i);
        t = hmac_sha256(&prk, &block).to_vec();
        out.extend_from_slice(&t);
        i += 1;
    }
    out.truncate(length);
    out
}

/// Constant-time comparison.
pub fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

fn aes_key(key: &[u8]) -> Result<LessSafeKey> {
    if key.len() != KEY_BYTES {
        return Err(CryptoError("key: 32 bytes"));
    }
    Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).map_err(|_| CryptoError("key"))?))
}

fn nonce_of(nonce: &[u8]) -> Result<Nonce> {
    Nonce::try_assume_unique_for_key(nonce).map_err(|_| CryptoError("nonce: 12 bytes"))
}

/// AES-256-GCM, the 16-byte tag after the ciphertext.
pub fn aes_seal(key: &[u8], nonce: &[u8], aad: &str, plain: &[u8]) -> Result<Vec<u8>> {
    let k = aes_key(key)?;
    let mut buf = plain.to_vec();
    k.seal_in_place_append_tag(nonce_of(nonce)?, Aad::from(aad.as_bytes()), &mut buf)
        .map_err(|_| CryptoError("can't encrypt"))?;
    Ok(buf)
}

pub fn aes_open(key: &[u8], nonce: &[u8], aad: &str, sealed: &[u8]) -> Result<Vec<u8>> {
    let k = aes_key(key)?;
    let mut buf = sealed.to_vec();
    let plain = k
        .open_in_place(nonce_of(nonce)?, Aad::from(aad.as_bytes()), &mut buf)
        .map_err(|_| CryptoError("can't decrypt"))?;
    Ok(plain.to_vec())
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).expect("writing to memory");
    e.finish().expect("writing to memory")
}

/// CRC-32, the gzip trailer's.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &x in bytes {
        c ^= x as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    c ^ 0xffff_ffff
}

/// The length of a gzip member's header (RFC 1952: FEXTRA, FNAME, FCOMMENT, FHCRC), or None when it isn't one.
fn gzip_header_len(b: &[u8]) -> Option<usize> {
    if b.len() < 18 || b[0] != 0x1f || b[1] != 0x8b || b[2] != 8 || b[3] & 0xe0 != 0 {
        return None;
    }
    let flg = b[3];
    let mut i = 10usize;
    if flg & 4 != 0 {
        i += 2 + (b[i] as usize | (b[i + 1] as usize) << 8);
    }
    for bit in [8u8, 16] {
        if flg & bit != 0 {
            while i < b.len() {
                i += 1;
                if b[i - 1] == 0 {
                    break;
                }
            }
        }
    }
    if flg & 2 != 0 {
        i += 2;
    }
    (i < b.len()).then_some(i)
}

/// Inflates exactly one gzip member, at most `max` bytes (§3.2): its deflate stream must be the whole of what lies between the
/// header and the last 8 bytes, and those must be the output's CRC-32 and length. Trailing bytes and second members are refused.
pub fn gunzip(b: &[u8], max: usize) -> Result<Vec<u8>> {
    let not = || CryptoError("not gzip");
    let h = gzip_header_len(b).ok_or_else(not)?;
    if b.len() < h + 8 {
        return Err(not());
    }
    let body = &b[h..b.len() - 8];
    let mut d = flate2::read::DeflateDecoder::new(body);
    let mut out = Vec::new();
    (&mut d).take(max as u64 + 1).read_to_end(&mut out).map_err(|_| not())?;
    if out.len() > max {
        return Err(CryptoError("too big"));
    }
    if d.total_in() as usize != body.len() {
        return Err(not());
    }
    let t = &b[b.len() - 8..];
    let crc = u32::from_le_bytes([t[0], t[1], t[2], t[3]]);
    let len = u32::from_le_bytes([t[4], t[5], t[6], t[7]]);
    if crc != crc32(&out) || len != out.len() as u32 {
        return Err(not());
    }
    Ok(out)
}

/// What travels: epoch, nonce, ciphertext and tag; a key envelope adds `p`, the sender's ephemeral public key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub e: i64,
    pub n: String,
    pub c: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p: Option<String>,
}

/// `stash-sync/1|<spaceId>|<epoch>|<place>`; pairing uses the space id `pair` and epoch 0.
pub fn aad(space_id: &str, epoch: i64, place: &str) -> String {
    format!("{AAD_PREFIX}|{space_id}|{epoch}|{place}")
}

/// The places of §3.2.
pub mod place {
    pub const LOG: &str = "log";
    pub const CONFIG: &str = "config";
    pub fn now(device: &str) -> String {
        format!("now:{device}")
    }
    pub fn queue(device: &str) -> String {
        format!("queue:{device}")
    }
    pub fn label(device: &str) -> String {
        format!("label:{device}")
    }
    pub fn key(device: &str) -> String {
        format!("key:{device}")
    }
    pub fn pair_answer(pair_id: &str) -> String {
        format!("answer:{pair_id}")
    }
    pub fn pair_reply(pair_id: &str) -> String {
        format!("reply:{pair_id}")
    }
    pub fn remote(device: &str) -> String {
        format!("remote:{device}")
    }
    pub fn remote_cmd(from: &str, to: &str) -> String {
        format!("remote-cmd:{from}:{to}")
    }
}

/// A space's data key: HKDF(K, salt = UTF-8 spaceId, info = "stash-sync v1 data").
pub fn data_key(space_key: &[u8], space_id: &str) -> Vec<u8> {
    hkdf(space_key, space_id.as_bytes(), INFO_DATA.as_bytes(), KEY_BYTES)
}

/// Seals gzip bytes already made, with the nonce given: the golden tests' entry. App code calls [seal].
pub fn seal_gz_with(key: &[u8], space_id: &str, epoch: i64, place: &str, gz: &[u8], nonce: &[u8]) -> Result<Envelope> {
    let c = aes_seal(key, nonce, &aad(space_id, epoch, place), gz)?;
    Ok(Envelope { e: epoch, n: b64u(nonce), c: b64u(&c), p: None })
}

/// gzip(UTF-8 JSON) sealed under `key` for (spaceId, epoch, place), with a fresh random nonce.
pub fn seal(key: &[u8], space_id: &str, epoch: i64, place: &str, doc: &Value) -> Result<Envelope> {
    let json = serde_json::to_vec(doc).map_err(|_| CryptoError("not JSON"))?;
    seal_gz_with(key, space_id, epoch, place, &gzip(&json), &random_bytes(NONCE_BYTES))
}

/// The plaintext bytes of an envelope sealed for (spaceId, place), unparsed. The epoch in the AAD is the envelope's own.
pub fn open_raw(key: &[u8], space_id: &str, place: &str, env: &Envelope) -> Result<Vec<u8>> {
    if env.e < 0 {
        return Err(CryptoError("bad envelope"));
    }
    let n = unb64(&env.n).map_err(|_| CryptoError("bad envelope"))?;
    let c = unb64(&env.c).map_err(|_| CryptoError("bad envelope"))?;
    gunzip(&aes_open(key, &n, &aad(space_id, env.e, place), &c)?, MAX_PLAINTEXT)
}

/// Opens an envelope sealed for (spaceId, place) and parses its JSON.
pub fn open(key: &[u8], space_id: &str, place: &str, env: &Envelope) -> Result<Value> {
    let plain = open_raw(key, space_id, place, env)?;
    let text = std::str::from_utf8(&plain).map_err(|_| CryptoError("bad plaintext"))?;
    serde_json::from_str(text).map_err(|_| CryptoError("bad plaintext"))
}

/// A device token's server-side form: base64url(SHA-256(the token's bytes)).
pub fn token_hash(token: &[u8]) -> String {
    b64u(&sha256(token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_is_the_rfc_s() {
        // RFC 5869 test case 1.
        let okm = hkdf(
            &[0x0b; 22],
            &(0u8..=12).collect::<Vec<_>>(),
            &(0xf0u8..=0xf9).collect::<Vec<_>>(),
            42,
        );
        assert_eq!(
            okm.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn gunzip_refuses_trailing_bytes_and_a_second_member() {
        let one = gzip(b"hello");
        assert_eq!(gunzip(&one, 100).unwrap(), b"hello");
        let mut trailing = one.clone();
        trailing.push(0);
        assert!(gunzip(&trailing, 100).is_err());
        let mut two = gzip(b"");
        two.extend_from_slice(&one);
        assert!(gunzip(&two, 100).is_err());
        assert_eq!(gunzip(&gzip(&[7u8; 500]), 100), Err(CryptoError("too big")));
    }
}
