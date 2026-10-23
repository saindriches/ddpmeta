//! Re-signing: protection regenerated from the frame data, idempotence, stale repair, a
//! missing key_id, streams without protection, and the in-place path.

mod common;

use std::path::Path;

use common::{data, frames, read, DDP};
use ddpmeta::bits;
use ddpmeta::emdf;
use ddpmeta::emdf_protection::{KeySet, ProtectionKey};
use ddpmeta::error::Error;
use ddpmeta::frame::{fix_crcs, Codec};
use ddpmeta::protect::{self, Integrity};
use ddpmeta::resign::{resign, resign_in_place};

/// Keys for both key_id values in use, unrelated to the keys the fixtures were signed with.
fn keys() -> KeySet {
    let mut k = KeySet::new();
    k.insert(0, ProtectionKey::new(vec![0x11; 32]).unwrap());
    k.insert(1, ProtectionKey::new(vec![0x22; 32]).unwrap());
    k
}

/// Flip one primary protection byte of the first container, then fix the frame CRC.
fn corrupt_protection(data: &mut [u8]) {
    let (p, len) = {
        let (f, d) = frames(data).into_iter().next().unwrap();
        let c = emdf::containers(&f, d).remove(0);
        let p = c.protection.as_ref().expect("protected container");
        (c.frame_pos(p.body_pos), f.bytes)
    };
    bits::set(data, p, 8, bits::get(data, p, 8) ^ 0xff);
    fix_crcs(Codec::Eac3, &mut data[..len]);
}

#[test]
fn resign_recomputes_protection_and_is_idempotent() {
    let k = keys();
    for name in ["ddp51.ec3", "bd71.ec3", "jocbd.ec3"] {
        let d = data(name);
        let (out, rep) = resign(&d, &k).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(out.len(), d.len(), "{name}");
        assert!(rep.containers_resigned > 0, "{name}");
        for (f, x) in frames(&out) {
            for c in emdf::containers(&f, x) {
                if c.protection.is_some() {
                    assert_eq!(
                        protect::verify(&f, x, &c, Some(&k)),
                        Integrity::Valid,
                        "{name}"
                    );
                }
            }
        }
        let (again, rep2) = resign(&out, &k).unwrap();
        assert_eq!(again, out, "{name}: second pass changed the stream");
        assert_eq!(rep2.frames_changed, 0, "{name}");
        assert_eq!(rep2.containers_resigned, rep.containers_resigned, "{name}");
    }
}

#[test]
fn resign_repairs_stale_protection_from_the_frame_data() {
    let k = keys();
    let d = read(DDP);
    let (clean, _) = resign(&d, &k).unwrap();
    let mut stale = clean.clone();
    corrupt_protection(&mut stale);
    assert_ne!(stale, clean);

    let (out, rep) = resign(&stale, &k).unwrap();
    assert_eq!(out, clean);
    assert_eq!(rep.frames_changed, 1);
    assert_eq!(rep.containers_resigned, 32);
}

#[test]
fn resign_refuses_a_container_without_its_key() {
    let mut k = KeySet::new();
    k.insert(0, ProtectionKey::new(vec![0x11; 32]).unwrap());
    // bd71.ec3 carries key_id 1 containers in the dependent substream.
    match resign(&data("bd71.ec3"), &k) {
        Err(Error::Protected(m)) => assert!(m.contains("no key configured for key_id 1"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn resign_leaves_an_unprotected_stream_alone() {
    let d = data("dd51.ac3");
    let (out, rep) = resign(&d, &keys()).unwrap();
    assert_eq!(out, d);
    assert!(rep.frames > 0);
    assert_eq!(rep.frames_changed, 0);
    assert_eq!(rep.containers_resigned, 0);
}

#[test]
fn resign_in_place_repairs_the_file() {
    let k = keys();
    let d = read(DDP);
    let (clean, _) = resign(&d, &k).unwrap();
    let mut stale = clean.clone();
    corrupt_protection(&mut stale);

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("resign_in_place");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("in.ec3");
    std::fs::write(&path, &stale).unwrap();

    let rep = resign_in_place(&path, &k).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), clean);
    assert_eq!(rep.frames_changed, 1);
    assert_eq!(rep.containers_resigned, 32);
}
