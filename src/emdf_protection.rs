//! Key file loading and container blanking. The protection rule and the key types are in
//! `emdf_protection_core`, one implementation for DD+ and DD+ JOC.
//!
//! Rule: with protection length codes 2 and 1,
//!   primary   = first 4 bytes of HMAC-SHA256(key, core || container with both protection
//!               fields zeroed),
//!   secondary = first byte of HMAC-SHA256(key, the full 32 byte primary digest).
//! Nothing carries over from earlier frames, so every container verifies on its own.
//!
//! The key is configuration: it is never stored in code, tests, fixtures or git, and never
//! printed. ETSI TS 102 366 V1.4.1 H.2.2.4.3 leaves the calculation implementation dependent.

use crate::bits;
use crate::error::{Error, Result};

pub use crate::emdf_protection_core::{authenticated_core, protect, KeySet, ProtectionKey};

/// Environment variable naming the key file.
pub const KEY_FILE_ENV: &str = "EMDF_PROTECTION_KEY_FILE";

/// Load the keys from an explicit file, else from the file named by `KEY_FILE_ENV`, else (only
/// in a build with the `embed-key` feature) from the key set embedded at build time.
///
/// A key file is the machine binary [`KeySet`] form (keys indexed by `key_id`). A file that does
/// not start with the binary magic is read as one key: whitespace stripped, decoded as hex, and
/// taken as `key_id` 0.
pub fn load_keys(path: Option<&str>) -> Result<Option<KeySet>> {
    let p = match path {
        Some(p) => p.to_string(),
        None => match std::env::var(KEY_FILE_ENV) {
            Ok(p) if !p.is_empty() => p,
            _ => return embedded_keys(),
        },
    };
    let bytes = std::fs::read(&p).map_err(|e| Error::Io(format!("EMDF key file {p}: {e}")))?;
    parse_keys(&bytes).map_err(|e| Error::Invalid(format!("EMDF key file {p}: {e}")))
}

/// Parse the machine binary [`KeySet`], or a bare-hex file as the single `key_id` 0.
fn parse_keys(bytes: &[u8]) -> Result<Option<KeySet>> {
    if bytes.starts_with(&KeySet::MAGIC) {
        return KeySet::from_binary(bytes)
            .map(Some)
            .map_err(|e| Error::Invalid(e.to_string()));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Invalid("not the binary form and not hex".into()))?;
    let hex: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let key = ProtectionKey::from_hex(&hex).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut set = KeySet::new();
    set.insert(0, key);
    Ok(Some(set))
}

/// The key set embedded at build time, used only in a `--features embed-key` build and only when
/// no key file is configured. `build.rs` writes it to OUT_DIR from `EMDF_EMBED_KEY_B64` (the
/// binary key file, base64) or `EMDF_EMBED_KEY_FILE` (its path), never from source or git; a
/// build without the feature has no embedded key.
#[cfg(feature = "embed-key")]
const EMBEDDED_KEY_FILE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded_keys.bin"));

#[cfg(feature = "embed-key")]
fn embedded_keys() -> Result<Option<KeySet>> {
    parse_keys(EMBEDDED_KEY_FILE).map_err(|e| Error::Invalid(format!("embedded EMDF key set: {e}")))
}

#[cfg(not(feature = "embed-key"))]
fn embedded_keys() -> Result<Option<KeySet>> {
    Ok(None)
}

/// Copy of a container body (its bytes after the EMDF sync, `Container::body`) with the
/// `prot_bits` protection bits from body offset `prot_pos` zeroed.
pub fn blank_container(body: &[u8], prot_pos: usize, prot_bits: usize) -> Vec<u8> {
    let mut b = body.to_vec();
    let mut i = 0;
    while i < prot_bits {
        let k = (prot_bits - i).min(32);
        bits::set(&mut b, prot_pos + i, k, 0);
        i += k;
    }
    b
}
