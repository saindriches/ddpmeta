//! Which syncframe bits an EMDF protection value authenticates, and verification and
//! re-signing of protected containers. ETSI TS 102 366 V1.4.1 H.2.2.4.3 leaves the protection
//! calculation implementation dependent; the rule here reproduces the protection of every
//! stream at hand.
//!
//! Everything is authenticated except the sync word with strmtyp, substreamid and frmsiz; the
//! informational metadata block (infomdate through sourcefscod); addbsie, addbsil and addbsi;
//! skipflde; every block's skiple, skipl and skipfld; and everything from the end of the last
//! audio block to the end of the frame (auxbits including the trailing word, auxdatal, auxdatae,
//! encinfo, crc2). AC-3 has no established rule: no AC-3 stream at hand carries EMDF.
//!
//! The same rule, applied to the dependent substream frame alone, reproduces the stored
//! primary and secondary values of every container in the E-AC-3 dependent substream
//! (strmtyp 1) of the Blu-ray JOC streams: 16 of 16 in tests/data/joc_bluray.ec3 and 16 of 16 in
//! tests/data/jocbd.ec3, including the 15 and 10 containers that continue across skip fields
//! (tests/corpus.rs, with the key configured).
//!
//! TS 102 366 V1.4.1 H.2.2.2.3 leaves key_id values implementation dependent. Two are in use,
//! with distinct keys: key_id 0 in streams whose loudness was corrected (and in JOC), key_id 1
//! in streams whose loudness was only measured (every Blu-ray 7.1 stream at hand).
//!
//! The keys are configuration (`emdf_protection::load_keys`, a [`KeySet`] indexed by `key_id`).
//! A container is verified or re-signed with the key for its own `key_id`; one with no
//! configured key is reported unverified and left unsigned. Nothing here stores or prints a
//! key.

use crate::bits;
use crate::emdf::{self, Container};
use crate::emdf_protection::{authenticated_core, blank_container, protect, KeySet};
use crate::frame::{Codec, Frame};
use crate::syntax::Role;

const INFO_FIELDS: [&str; 16] = [
    "infomdate",
    "bsmod",
    "copyrightb",
    "origbs",
    "dsurmod",
    "dheadphonmod",
    "dsurexmod",
    "audprodie",
    "mixlevel",
    "roomtyp",
    "adconvtyp",
    "audprodi2e",
    "mixlevel2",
    "roomtyp2",
    "adconvtyp2",
    "sourcefscod",
];

/// Excluded INCLUSIVE bit ranges `(first, last)`, merged and sorted. `None` when no rule is
/// established for the frame's syntax: the streams at hand cover E-AC-3 independent (strmtyp 0)
/// and dependent (strmtyp 1) substreams.
pub fn excluded(frame: &Frame) -> Option<Vec<(usize, usize)>> {
    if frame.codec != Codec::Eac3 || frame.strmtyp > 1 {
        return None;
    }
    let mut r = vec![(0usize, 32usize)];
    let mut in_info = false;
    for f in &frame.fields {
        if f.block >= 0 {
            break;
        }
        if f.name == "infomdate" {
            in_info = true;
        }
        if (in_info && INFO_FIELDS.contains(&f.name))
            || matches!(f.name, "addbsie" | "addbsil" | "addbsi" | "skipflde")
        {
            r.push((f.pos, f.end()));
        }
    }
    for f in &frame.fields {
        if matches!(f.role, Role::Skiple | Role::Skipl | Role::Skipfld) {
            r.push((f.pos, f.end()));
        }
    }
    r.push((frame.audio_end, frame.total_bits()));
    r.retain(|&(a, b)| b > a);
    r.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, b) in r {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    Some(merged.into_iter().map(|(a, b)| (a, b - 1)).collect())
}

/// The authenticated core of a frame under the rule above.
pub fn authenticated(frame: &Frame, data: &[u8]) -> Option<Vec<u8>> {
    authenticated_core(data, &excluded(frame)?).ok()
}

/// True when bit `p` is authenticated (`ex` from `excluded`).
pub fn is_authenticated(ex: &[(usize, usize)], p: usize) -> bool {
    !ex.iter().any(|&(a, b)| a <= p && p <= b)
}

/// Integrity of one protected container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Valid,
    Invalid { primary: bool, secondary: bool },
    Unverified(String),
}

impl std::fmt::Display for Integrity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = |ok: bool| if ok { "valid" } else { "invalid" };
        match self {
            Integrity::Valid => write!(f, "primary valid, secondary valid"),
            Integrity::Invalid { primary, secondary } => {
                write!(
                    f,
                    "primary {}, secondary {}",
                    word(*primary),
                    word(*secondary)
                )
            }
            Integrity::Unverified(why) => write!(f, "unverified, {why}"),
        }
    }
}

/// Protection values the rule gives for one container, or why they cannot be computed.
pub fn expected(
    frame: &Frame,
    data: &[u8],
    c: &Container,
    keys: &KeySet,
) -> Result<[u8; 5], String> {
    if let Some(e) = &c.error {
        return Err(format!("container undecodable ({e})"));
    }
    let Some(p) = &c.protection else {
        return Err("container has no protection".into());
    };
    let Some(key) = keys.get(c.key_id) else {
        return Err(format!("no key configured for key_id {}", c.key_id));
    };
    if p.primary_bits != 32 || p.secondary_bits != 8 {
        return Err(
            "rule unknown (only 32 bit primary with 8 bit secondary is established)".into(),
        );
    }
    let Some(core) = authenticated(frame, data) else {
        return Err("rule unknown for this syntax".into());
    };
    let blank = blank_container(&c.body, p.body_pos, 40);
    Ok(protect(key, &core, &blank))
}

/// The five stored protection bytes (primary then secondary) of a protected container.
pub fn stored(c: &Container) -> Option<[u8; 5]> {
    let p = c.protection.as_ref()?;
    let mut v = [0u8; 5];
    for (i, b) in v.iter_mut().enumerate() {
        *b = bits::get(&c.body, p.body_pos + 8 * i, 8) as u8;
    }
    Some(v)
}

/// Verify one container.
pub fn verify(frame: &Frame, data: &[u8], c: &Container, keys: Option<&KeySet>) -> Integrity {
    if c.protection.is_none() {
        return Integrity::Unverified("container has no protection".into());
    }
    let Some(keys) = keys else {
        return Integrity::Unverified("no key".into());
    };
    match expected(frame, data, c, keys) {
        Err(why) => Integrity::Unverified(why),
        Ok(v) => {
            let s = stored(c).expect("protected");
            let primary = s[..4] == v[..4];
            let secondary = s[4] == v[4];
            if primary && secondary {
                Integrity::Valid
            } else {
                Integrity::Invalid { primary, secondary }
            }
        }
    }
}

/// Recompute and write the protection of every protected container of a frame. Returns the
/// number of containers re-signed. The frame's CRCs must be fixed afterwards, since the
/// protection bits lie inside the CRC coverage. Writes nothing and returns the reason when any
/// container is undecodable or cannot be signed with the key.
pub fn resign(frame: &Frame, data: &mut [u8], keys: &KeySet) -> Result<usize, String> {
    let containers = emdf::containers(frame, data);
    let mut values = Vec::new();
    for c in &containers {
        if c.error.is_none() && c.protection.is_none() {
            continue;
        }
        values.push((c, expected(frame, data, c, keys)?));
    }
    for (c, v) in &values {
        let p = c.protection.as_ref().expect("checked by expected");
        for (i, &b) in v.iter().enumerate() {
            c.write(data, p.body_pos + 8 * i, 8, u32::from(b));
        }
    }
    Ok(values.len())
}
