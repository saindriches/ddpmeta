#![allow(dead_code)]

use std::path::{Path, PathBuf};

use ddpmeta::bits::{self, Writer};
use ddpmeta::emdf_protection::{load_keys, KeySet, KEY_FILE_ENV};
use ddpmeta::frame::{fix_crcs, Frame};
use ddpmeta::stream::{parse_frame, split};
use ddpmeta::syntax::Role;

pub const DDP: &str = "tests/data/evidence/pcm-ingress-v1-plain.ec3";
pub const JOC: &str = "tests/data/evidence/joc-params-rate384-getter-v1-plain.ec3";

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

pub fn read(rel: &str) -> Vec<u8> {
    std::fs::read(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Every .ac3 and .ec3 file under tests/data/evidence, as paths relative to the crate root.
pub fn evidence_streams() -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("ac3" | "ec3")) {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    let base = root();
    walk(&base.join("tests/data/evidence"), &base, &mut out);
    out.sort();
    out
}

/// The configured EMDF keys (indexed by key_id), or None (tests that need them then skip).
/// Never printed.
pub fn key() -> Option<KeySet> {
    if std::env::var(KEY_FILE_ENV).map_or(true, |v| v.is_empty()) {
        eprintln!("skipping key dependent checks: {KEY_FILE_ENV} is not set");
        return None;
    }
    load_keys(None).expect("key file configured but unreadable")
}

pub fn frames(data: &[u8]) -> Vec<(Frame, &[u8])> {
    split(data)
        .unwrap()
        .into_iter()
        .map(|(at, len)| {
            let d = &data[at..at + len];
            (parse_frame(d).unwrap(), d)
        })
        .collect()
}

/// Elements that exist only when strmtyp is 0 (Annex E, E.1.2.2 to E.1.2.4), for frames with
/// mixdef 0 and no panning or block mixing configuration.
const STRMTYP0_ONLY: [&str; 12] = [
    "pgmscle",
    "pgmscl",
    "pgmscl2e",
    "pgmscl2",
    "extpgmscle",
    "extpgmscl",
    "mixdef",
    "frmmixcfginfoe",
    "convsync",
    "convexpstre",
    "convexpstr",
    "convsnroffste",
];

/// Synthetic test fixture: turn an independent DD+ syncframe into a dependent substream
/// syncframe of the same size (strmtyp 1, chanmape 0, the strmtyp 0 only elements removed,
/// skip fields and their EMDF container removed, the difference absorbed at the start of
/// auxbits). It is syntactically valid Annex E; no encoder produced it.
pub fn dependent_from(f: &Frame, d: &[u8]) -> Vec<u8> {
    assert_eq!(
        f.named("mixdef").map(|x| x.value),
        Some(0),
        "fixture needs mixdef 0"
    );
    assert!(f.named("blkmixcfginfoe").is_none() && f.named("paninfoe").is_none());
    let mut dropped = 0usize;
    let mut inserted = 0usize;
    let mut w = Writer::new();
    for fl in &f.fields {
        if fl.name == "mixmdate" {
            w.put(1, 0); // chanmape
            inserted += 1;
        }
        let drop = STRMTYP0_ONLY.contains(&fl.name)
            || fl.name == "convsnroffst"
            || matches!(fl.role, Role::Skipl | Role::Skipfld);
        if drop {
            dropped += fl.width;
            continue;
        }
        match fl.role {
            _ if fl.name == "strmtyp" => w.put(2, 1),
            Role::Skiple => w.put(1, 0),
            Role::AuxBits => {
                assert!(dropped >= inserted);
                w.zeros(dropped - inserted);
                w.copy(d, fl.pos, fl.width);
            }
            _ => w.copy(d, fl.pos, fl.width),
        }
    }
    let mut out = w.into_bytes();
    assert_eq!(out.len(), f.bytes);
    fix_crcs(f.codec, &mut out);
    let g = parse_frame(&out).expect("synthetic dependent frame parses");
    assert_eq!(g.strmtyp, 1);
    assert!(bits::range_zero(
        &out,
        g.find(Role::AuxBits, -1).unwrap().pos,
        dropped - inserted
    ));
    out
}

/// Independent DD+ frames each followed by a synthetic dependent substream frame.
pub fn with_dependents(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (f, d) in frames(data) {
        out.extend_from_slice(d);
        out.extend_from_slice(&dependent_from(&f, d));
    }
    out
}

/// A stream from tests/data.
pub fn data(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}
