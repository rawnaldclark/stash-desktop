//! sync-v1 keys (`docs/sync-v1.md` §3.4–3.6), as the web player's `keys.ts`: P-256 device and ephemeral keys, labels, the QR
//! link, Kpair and the six-digit code, what a pairing grants, and opening a key envelope after a rotation. This computer plays
//! the part the browser plays in a pairing: it shows the code and the phone answers it.

use p256::elliptic_curve::sec1::ToEncodedPoint as _;
use p256::{PublicKey, SecretKey};
use serde_json::{Value, json};

use super::crypto::{CryptoError, Envelope, KEY_BYTES, Result, b64u, hkdf, hmac_sha256, open, place, same_bytes, unb64};

pub const INFO_PAIR_LABEL: &str = "stash-sync pair label";
pub const INFO_PAIR: &str = "stash-sync pair v1";
pub const INFO_SAS: &str = "stash-sync pair sas";
pub const INFO_KEY: &str = "stash-sync key v1";
pub const INFO_ROTATE: &str = "stash-sync rotate v1";
/// The QR's link, before its `#` fragment.
pub const LINK_BASE: &str = "https://stashfm.app/link";
/// A device's name is cut to this many characters.
pub const MAX_LABEL: usize = 60;

fn is_device_id(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A P-256 key pair: the 32-byte secret and the 65-byte uncompressed public point.
#[derive(Clone)]
pub struct KeyPair {
    secret: SecretKey,
    pub public: Vec<u8>,
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        loop {
            if let Ok(k) = KeyPair::from_secret(&super::crypto::random_bytes(32)) {
                return k;
            }
        }
    }

    /// From its 32-byte secret (kept on this computer, or a test vector's).
    pub fn from_secret(d: &[u8]) -> Result<KeyPair> {
        let secret = SecretKey::from_slice(d).map_err(|_| CryptoError("not a P-256 key"))?;
        let public = secret.public_key().to_encoded_point(false).as_bytes().to_vec();
        Ok(KeyPair { secret, public })
    }

    pub fn secret_bytes(&self) -> Vec<u8> {
        self.secret.to_bytes().to_vec()
    }

    /// ECDH with another key: the shared point's X coordinate, 32 bytes (what WebCrypto and the JCA return).
    pub fn ecdh(&self, their_pub: &[u8]) -> Result<Vec<u8>> {
        if !is_pub(their_pub) {
            return Err(CryptoError("public key: 65 bytes, uncompressed"));
        }
        let theirs = PublicKey::from_sec1_bytes(their_pub).map_err(|_| CryptoError("public key: not on P-256"))?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), theirs.as_affine());
        Ok(shared.raw_secret_bytes().to_vec())
    }
}

/// An uncompressed public point: 65 bytes, 0x04 first.
pub fn is_pub(raw: &[u8]) -> bool {
    raw.len() == 65 && raw[0] == 4
}

/// A device's name, type and long-term key, as its owner wrote it (inside the pair secret or the space key, never in clear).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    /// `phone`, `web` or `desktop`.
    pub kind: String,
    pub device_id: String,
    pub public: Vec<u8>,
}

/// A name as it may be shown: control and direction-changing characters dropped, whitespace runs as one space, trimmed.
pub fn plain_name(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !(u <= 0x1f
                || (0x7f..=0x9f).contains(&u)
                || u == 0x061c
                || (0x200b..=0x200f).contains(&u)
                || (0x202a..=0x202e).contains(&u)
                || (0x2060..=0x2069).contains(&u)
                || u == 0xfeff)
        })
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn label_doc(l: &Label) -> Value {
    json!({ "kind": "stash-label", "v": 1, "name": l.name, "type": l.kind, "deviceId": l.device_id, "pub": b64u(&l.public) })
}

/// A `stash-label` read back: it must name `device_id` (where it was found) and carry a P-256 point. The only trusted source
/// of another device's key.
pub fn read_label(doc: &Value, device_id: &str) -> Result<Label> {
    let bad = || CryptoError("bad label");
    if doc.get("kind").and_then(Value::as_str) != Some("stash-label") {
        return Err(bad());
    }
    if doc.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(CryptoError("newer label"));
    }
    let kind = doc.get("type").and_then(Value::as_str).ok_or_else(bad)?;
    if !matches!(kind, "phone" | "web" | "desktop") || doc.get("deviceId").and_then(Value::as_str) != Some(device_id) {
        return Err(bad());
    }
    let name = doc.get("name").and_then(Value::as_str).ok_or_else(bad)?;
    let public = unb64(doc.get("pub").and_then(Value::as_str).ok_or_else(bad)?).map_err(|_| bad())?;
    if !is_pub(&public) {
        return Err(bad());
    }
    let mut name: String = plain_name(name).chars().take(MAX_LABEL).collect();
    if name.is_empty() {
        name = match kind {
            "phone" => "Phone",
            "desktop" => "Computer",
            _ => "Browser",
        }
        .into();
    }
    Ok(Label { name, kind: kind.into(), device_id: device_id.into(), public })
}

/// `https://stashfm.app/link#1.<pairId>.<pairSecret>.<eB.pub>`, every part base64url.
pub fn qr_link(pair_id: &str, pair_secret: &[u8], e_pub: &[u8]) -> String {
    format!("{LINK_BASE}#1.{pair_id}.{}.{}", b64u(pair_secret), b64u(e_pub))
}

/// The key both devices' pairing labels are sealed with.
pub fn pair_label_key(pair_secret: &[u8]) -> Vec<u8> {
    hkdf(pair_secret, &[], INFO_PAIR_LABEL.as_bytes(), KEY_BYTES)
}

/// Kpair's HKDF info: `stash-sync pair v1|<pairId>|<eB.pub>|<eP.pub>|<shower's deviceId>|<shower's pub>`.
pub fn pair_info(pair_id: &str, e_b_pub: &[u8], e_p_pub: &[u8], shower_id: &str, shower_pub: &[u8]) -> String {
    format!("{INFO_PAIR}|{pair_id}|{}|{}|{shower_id}|{}", b64u(e_b_pub), b64u(e_p_pub), b64u(shower_pub))
}

/// Kpair = HKDF(ECDH(eB, eP.pub), salt = pairSecret, info = pair_info), on this computer's side (it showed the code).
pub fn pair_key(
    e_b: &KeyPair,
    e_p_pub: &[u8],
    pair_secret: &[u8],
    pair_id: &str,
    shower_id: &str,
    shower_pub: &[u8],
) -> Result<Vec<u8>> {
    let shared = e_b.ecdh(e_p_pub)?;
    let info = pair_info(pair_id, &e_b.public, e_p_pub, shower_id, shower_pub);
    Ok(hkdf(&shared, pair_secret, info.as_bytes(), KEY_BYTES))
}

/// The six digits both screens show: HKDF(Kpair, "stash-sync pair sas", 4 bytes), big-endian, mod 1,000,000, zero-padded.
pub fn pair_code(kpair: &[u8]) -> String {
    let b = hkdf(kpair, &[], INFO_SAS.as_bytes(), 4);
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    format!("{:06}", n % 1_000_000)
}

/// What a pairing grants: a space, its key and epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceGrant {
    pub id: String,
    pub k: Vec<u8>,
    pub epoch: i64,
}

pub fn read_space_grant(x: &Value) -> Result<SpaceGrant> {
    let bad = || CryptoError("bad space");
    let id = x.get("id").and_then(Value::as_str).ok_or_else(bad)?;
    let tail = id.strip_prefix("s_").ok_or_else(bad)?;
    if tail.len() != 22 || unb64(tail).map_err(|_| bad())?.len() != 16 {
        return Err(bad());
    }
    let k = unb64(x.get("k").and_then(Value::as_str).ok_or_else(bad)?).map_err(|_| bad())?;
    let epoch = x.get("epoch").and_then(Value::as_i64).ok_or_else(bad)?;
    if k.len() != KEY_BYTES || epoch < 1 {
        return Err(bad());
    }
    Ok(SpaceGrant { id: id.into(), k, epoch })
}

/// A `stash-pair-answer` read back: the phone's label, and the space it grants.
pub fn read_pair_answer(doc: &Value) -> Result<(Label, Option<SpaceGrant>)> {
    let bad = || CryptoError("bad answer");
    if doc.get("kind").and_then(Value::as_str) != Some("stash-pair-answer") {
        return Err(bad());
    }
    if doc.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(CryptoError("newer answer"));
    }
    let id = doc.get("deviceId").and_then(Value::as_str).filter(|s| is_device_id(s)).ok_or_else(bad)?;
    let label = read_label(doc.get("label").ok_or_else(bad)?, id)?;
    if label.kind != "phone" {
        return Err(bad());
    }
    let space = match doc.get("space") {
        None | Some(Value::Null) => None,
        Some(s) => Some(read_space_grant(s)?),
    };
    Ok((label, space))
}

/// A key envelope's HKDF info: `stash-sync key v1|<spaceId>|<epoch>|<deviceId>|<ephemeral pub>|<device pub>`.
pub fn key_info(space_id: &str, epoch: i64, device_id: &str, eph_pub: &[u8], device_pub: &[u8]) -> String {
    format!("{INFO_KEY}|{space_id}|{epoch}|{device_id}|{}|{}", b64u(eph_pub), b64u(device_pub))
}

/// The proof key: HKDF(K of the previous epoch, salt = UTF-8 spaceId, info = "stash-sync rotate v1|<previous epoch>").
pub fn rotate_key(prev_key: &[u8], space_id: &str, prev_epoch: i64) -> Vec<u8> {
    hkdf(prev_key, space_id.as_bytes(), format!("{INFO_ROTATE}|{prev_epoch}").as_bytes(), KEY_BYTES)
}

/// What the proof covers: `<spaceId>|<epoch>|<deviceId>|<k>|<members joined by ",">`.
pub fn proof_input(space_id: &str, epoch: i64, device_id: &str, k: &[u8], members: &[String]) -> String {
    format!("{space_id}|{epoch}|{device_id}|{}|{}", b64u(k), members.join(","))
}

/// The next space key from a key envelope: only for exactly the next epoch, only when this device is among the members, and
/// only with a proof under the previous key (the server, which never had a key, can't forge one).
pub fn open_key_envelope(
    env: &Envelope,
    space_id: &str,
    device_id: &str,
    device: &KeyPair,
    prev_key: &[u8],
    prev_epoch: i64,
) -> Result<(Vec<u8>, i64, Vec<String>)> {
    let bad = || CryptoError("bad key document");
    if env.e != prev_epoch + 1 {
        return Err(CryptoError("wrong epoch"));
    }
    let eph = unb64(env.p.as_deref().ok_or_else(bad)?).map_err(|_| bad())?;
    let key = hkdf(
        &device.ecdh(&eph)?,
        space_id.as_bytes(),
        key_info(space_id, env.e, device_id, &eph, &device.public).as_bytes(),
        KEY_BYTES,
    );
    let d = open(&key, space_id, &place::key(device_id), env)?;
    if d.get("kind").and_then(Value::as_str) != Some("stash-key")
        || d.get("v").and_then(Value::as_i64) != Some(1)
        || d.get("epoch").and_then(Value::as_i64) != Some(env.e)
    {
        return Err(bad());
    }
    let members: Vec<String> = d
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
        .iter()
        .map(|m| m.as_str().filter(|s| is_device_id(s)).map(str::to_string).ok_or_else(bad))
        .collect::<Result<_>>()?;
    let mut sorted = members.clone();
    sorted.sort();
    sorted.dedup();
    if sorted != members || !sorted.iter().any(|m| m == device_id) {
        return Err(bad());
    }
    let k = unb64(d.get("k").and_then(Value::as_str).ok_or_else(bad)?).map_err(|_| bad())?;
    if k.len() != KEY_BYTES {
        return Err(bad());
    }
    let want = hmac_sha256(
        &rotate_key(prev_key, space_id, prev_epoch),
        proof_input(space_id, env.e, device_id, &k, &sorted).as_bytes(),
    );
    let proof = unb64(d.get("proof").and_then(Value::as_str).ok_or_else(bad)?).map_err(|_| bad())?;
    if !same_bytes(&want, &proof) {
        return Err(CryptoError("bad proof"));
    }
    Ok((k, env.e, sorted))
}
