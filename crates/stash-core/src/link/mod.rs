//! Linking Stash on this computer with the Stash app on the listener's phone (and their browsers), over the sync service
//! `https://sync.stashfm.app` (stash-player `docs/sync-v1.md`, MP3APK `infra/sync-worker`). This computer is a device of type
//! `desktop`: it shows a QR code, the phone answers it, both screens show the same six digits, and the computer joins the phone's
//! space. Every document is sealed on the device; the server sees ids, keys' public halves, sizes and times.
//!
//! - [`crypto`]: base64url, HKDF, AES-GCM, gzip and envelopes.
//! - [`keys`]: device keys, labels, the QR link, Kpair and the code, key envelopes.

pub mod crypto;
pub mod keys;
