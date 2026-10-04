//! Round trip, method A and method B on AC-3, DD+, DD+ JOC and a synthetic dependent
//! substream stream; CRCs, field identity, EMDF refusal and re-signing.

mod common;

use common::{data, frames, key, read, with_dependents, DDP, JOC};
use ddpmeta::bits;
use ddpmeta::edit::{apply, lower, Edit, Lowered, Method, Op, Strip};
use ddpmeta::emdf;
use ddpmeta::error::Error;
use ddpmeta::frame::{crc_status, fix_crcs, Codec, Frame};
use ddpmeta::protect::{self, Integrity};
use ddpmeta::show::show;
use ddpmeta::strip::{strip, Options};
use ddpmeta::syntax::Role;

fn opts(line: bool, rf: bool, dialnorm: bool, method: Method) -> Options {
    Options {
        strip: Strip { line, rf, dialnorm },
        method,
        key: None,
        allow_stale_protection: true,
    }
}

/// The encoder output streams of tests/data/evidence, each with its bytes.
fn aligned_streams() -> Vec<(String, Vec<u8>)> {
    common::evidence_streams()
        .into_iter()
        .map(|s| {
            let d = read(&s);
            (s, d)
        })
        .collect()
}

fn assert_crcs(data: &[u8]) {
    for (f, d) in frames(data) {
        let c = crc_status(f.codec, d);
        assert!(c.crc2 && c.crc1 != Some(false), "CRC {c:?}");
    }
}

#[test]
fn round_trip_rebuild_is_identical() {
    let streams = aligned_streams();
    assert!(
        streams.len() == 14,
        "expected the evidence streams, found {}",
        streams.len()
    );
    let mut n = 0;
    for (s, data) in &streams {
        for (f, d) in frames(data) {
            let keep = Lowered::Repack(vec![Op::Keep; f.fields.len()]);
            assert_eq!(apply(&f, d, &keep).unwrap(), d, "{s}: rebuild");
            let mut c = d.to_vec();
            fix_crcs(f.codec, &mut c);
            assert_eq!(c, d, "{s}: CRC recomputation");
            n += 1;
        }
    }
    eprintln!("{} streams, {n} frames rebuilt bit exact", streams.len());
}

fn drc_values(f: &Frame) -> Vec<(Role, i8, u32)> {
    f.fields
        .iter()
        .filter(|x| matches!(x.role, Role::Compr(_) | Role::Dynrng(_) | Role::Dialnorm(_)))
        .map(|x| (x.role, x.block, x.value))
        .collect()
}

#[test]
fn method_a_zeroes_words_in_place() {
    for (s, data) in aligned_streams() {
        let (out, rep) = strip(&data, &opts(true, true, true, Method::InPlace)).unwrap();
        assert_eq!(out.len(), data.len());
        assert_crcs(&out);
        for ((of, od), (nf, nd)) in frames(&data).into_iter().zip(frames(&out)) {
            assert_eq!(of.bytes, nf.bytes);
            // Same layout, same bits outside the DRC words, dialnorm, CRCs and protection.
            assert_eq!(of.fields.len(), nf.fields.len(), "{s}");
            for (a, b) in of.fields.iter().zip(&nf.fields) {
                assert_eq!((a.name, a.pos, a.width), (b.name, b.pos, b.width), "{s}");
                match a.role {
                    Role::Compr(_) | Role::Dynrng(_) => assert_eq!(b.value, 0),
                    Role::Dialnorm(_) => assert_eq!(b.value, 31),
                    Role::Crc1 | Role::Crc2 | Role::CrcRsv | Role::Skipfld => {}
                    _ if a.width <= 32 => assert_eq!(a.value, b.value, "{s} {}", a.name),
                    _ => assert!(
                        bits::range_eq(od, a.pos, nd, b.pos, a.width),
                        "{s} {}",
                        a.name
                    ),
                }
            }
        }
        assert_eq!(rep.words_removed, 0);
    }
}

#[test]
fn method_b_removes_words_and_keeps_every_other_field() {
    for (s, data) in aligned_streams() {
        let (out, rep) = strip(&data, &opts(true, true, true, Method::Repack)).unwrap();
        assert_eq!(out.len(), data.len());
        assert_crcs(&out);
        let mut freed_total = 0;
        for ((of, od), (nf, nd)) in frames(&data).into_iter().zip(frames(&out)) {
            assert_eq!(of.bytes, nf.bytes);
            // Independent check: the elements other than dynrng match one to one, by name and
            // value or content; every dynrng presence flag is 0; compr stays, set to 0x00.
            let keep = |f: &Frame| -> Vec<usize> {
                (0..f.fields.len())
                    .filter(|&i| !matches!(f.fields[i].role, Role::Dynrng(_)))
                    .collect()
            };
            let (ki, kn) = (keep(&of), keep(&nf));
            assert_eq!(ki.len(), kn.len(), "{s}");
            assert!(nf.all(Role::Dynrng(0)).next().is_none());
            assert_eq!(
                of.all(Role::Compr(0)).count(),
                nf.all(Role::Compr(0)).count(),
                "{s}"
            );
            let freed: usize = of
                .fields
                .iter()
                .filter(|x| matches!(x.role, Role::Dynrng(_)))
                .map(|x| x.width)
                .sum();
            freed_total += freed;
            for (&i, &j) in ki.iter().zip(&kn) {
                let (a, b) = (&of.fields[i], &nf.fields[j]);
                assert_eq!(
                    (a.name, a.block, a.index),
                    (b.name, b.block, b.index),
                    "{s}"
                );
                match a.role {
                    Role::DynrngExists(_) | Role::Compr(_) => assert_eq!(b.value, 0),
                    Role::Dialnorm(_) => assert_eq!(b.value, 31),
                    Role::Crc1 | Role::Crc2 | Role::CrcRsv | Role::Skipfld => {}
                    Role::AuxBits => {
                        assert_eq!(
                            b.width,
                            a.width + freed,
                            "{s}: auxbits grow by the freed bits"
                        );
                        assert_eq!(b.end(), a.end(), "{s}: aux data stays end anchored");
                        assert!(bits::range_zero(nd, b.pos, freed));
                        assert!(bits::range_eq(od, a.pos, nd, b.pos + freed, a.width));
                    }
                    _ if a.width <= 32 => assert_eq!(a.value, b.value, "{s} {}", a.name),
                    _ => assert!(
                        bits::range_eq(od, a.pos, nd, b.pos, a.width),
                        "{s} {}",
                        a.name
                    ),
                }
            }
        }
        assert_eq!(rep.freed_bits, freed_total);
    }
}

#[test]
fn edits_outside_the_strip_subset_are_not_supported_yet() {
    let data = read(DDP);
    let (f, _) = frames(&data).into_iter().next().unwrap();
    for e in [
        Edit::Field {
            name: "bsmod".into(),
            value: 1,
        },
        Edit::Dialnorm {
            program: 0,
            code: 20,
        },
        Edit::Compr {
            program: 0,
            code: Some(5),
        },
        Edit::Dynrng {
            program: 0,
            block: Some(1),
            code: Some(0),
        },
    ] {
        assert!(
            matches!(
                lower(&f, std::slice::from_ref(&e), Method::InPlace),
                Err(Error::NotSupportedYet(_))
            ),
            "{e:?}"
        );
    }
    // Removing words needs method B.
    let e = Edit::Dynrng {
        program: 0,
        block: None,
        code: None,
    };
    assert!(matches!(
        lower(&f, &[e], Method::InPlace),
        Err(Error::NotSupportedYet(_))
    ));
}

#[test]
fn rf_strip_keeps_compr_with_the_neutral_code() {
    // Varying compr and dynrng words.
    let data = data("ddp51drc.ec3");
    for method in [Method::InPlace, Method::Repack] {
        // --rf alone: compr 0xff, because 0x00 would make RF mode follow the kept dynrng.
        let (out, rep) = strip(&data, &opts(false, true, false, method)).unwrap();
        assert_crcs(&out);
        let not_ff = frames(&data)
            .iter()
            .filter(|(f, _)| f.value(Role::Compr(0), -1) != Some(0xff))
            .count();
        assert_eq!(not_ff, 72);
        assert_eq!(rep.compr_set_ff, not_ff, "{method:?} {rep:?}");
        assert!(rep.notes.iter().any(|n| n.contains("0xff")));
        for ((of, _), (nf, _)) in frames(&data).into_iter().zip(frames(&out)) {
            assert_eq!(nf.value(Role::Compr(0), -1), Some(0xff));
            let dyn_of: Vec<_> = of.all(Role::Dynrng(0)).map(|f| f.value).collect();
            let dyn_nf: Vec<_> = nf.all(Role::Dynrng(0)).map(|f| f.value).collect();
            assert_eq!(dyn_of, dyn_nf);
        }
        // --line --rf: compr 0x00, dynrng zeroed (A) or removed (B).
        let (out, rep) = strip(&data, &opts(true, true, false, method)).unwrap();
        assert_eq!(rep.compr_set_ff, 0);
        for (nf, _) in frames(&out) {
            assert_eq!(nf.value(Role::Compr(0), -1), Some(0));
            assert_eq!(nf.value(Role::ComprExists(0), -1), Some(1));
            assert!(nf.all(Role::Dynrng(0)).all(|f| f.value == 0));
        }
    }
    // Removing compr is left to the API, and refused where RF mode decoders would then use a
    // dynrng word other than 0 dB (6.7.3.1); the last plain.ec3 frame has dynrng 0x01.
    let plain = read(DDP);
    let e = [Edit::Compr {
        program: 0,
        code: None,
    }];
    assert!(frames(&plain).iter().any(|(f, _)| matches!(
        lower(f, &e, Method::Repack),
        Err(Error::NotSupportedYet(m)) if m.contains("6.7.3.1")
    )));
}

#[test]
fn dialnorm_edit_is_refused_without_a_key() {
    // Setting dialnorm to -31 dB also sets the programme loudness reference to -31 LUFS
    // (H.3.1.3.8), which the EMDF protection authenticates, so a protected stream needs a key.
    let data = read(DDP);
    let mut o = opts(false, false, true, Method::InPlace);
    o.allow_stale_protection = false;
    assert!(matches!(strip(&data, &o), Err(Error::Protected(_))));
}

#[test]
fn dialnorm_edit_sets_the_loudness_reference_and_re_signs() {
    let Some(k) = key() else { return };
    let data = read(DDP);
    let mut o = opts(false, false, true, Method::InPlace);
    o.key = Some(k.clone());
    o.allow_stale_protection = false;
    let (out, rep) = strip(&data, &o).expect("dialnorm strip with the key");
    assert!(rep.dialnorm_set > 0 && rep.loudness_set > 0);
    assert_eq!(rep.containers_stale, 0);
    assert!(rep.containers_resigned > 0);
    for (nf, nd) in frames(&out) {
        // dialnorm is -31 dB, and the gated loudness it must match is now -31 LUFS.
        if let Some(dn) = nf.value(Role::Dialnorm(0), -1) {
            assert_eq!(ddpmeta::gain::dialnorm_db(dn), -31);
        }
        for c in emdf::containers(&nf, nd) {
            if c.protection.is_some() {
                assert_eq!(protect::verify(&nf, nd, &c, Some(&k)), Integrity::Valid);
            }
            for p in &c.payloads {
                if let Some(lufs) = emdf::loudness(&c, p).and_then(|l| l.dialnorm_reference()) {
                    assert!((lufs + 31.0).abs() <= 0.5, "loudness {lufs} LUFS");
                }
            }
        }
    }
}

#[test]
fn protected_streams_are_refused_without_a_key() {
    for path in [DDP, JOC] {
        let data = read(path);
        for method in [Method::InPlace, Method::Repack] {
            let mut o = opts(true, true, false, method);
            o.allow_stale_protection = false;
            assert!(
                matches!(strip(&data, &o), Err(Error::Protected(_))),
                "{path} {method:?}"
            );
        }
        // With the explicit flag the protection values are left as they were (now stale).
        let (out, rep) = strip(&data, &opts(true, true, false, Method::InPlace)).unwrap();
        assert_eq!(rep.containers_stale, 32);
        for ((of, od), (nf, nd)) in frames(&data).into_iter().zip(frames(&out)) {
            let a = emdf::containers(&of, od);
            let b = emdf::containers(&nf, nd);
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(&b) {
                assert_eq!(protect::stored(x), protect::stored(y));
            }
        }
    }
}

#[test]
fn input_protection_verifies_with_the_configured_key() {
    let Some(k) = key() else { return };
    for path in [DDP, JOC] {
        let data = read(path);
        let mut n = 0;
        for (f, d) in frames(&data) {
            for c in emdf::containers(&f, d) {
                assert_eq!(
                    protect::verify(&f, d, &c, Some(&k)),
                    Integrity::Valid,
                    "{path}"
                );
                n += 1;
            }
        }
        assert_eq!(n, 32, "{path}");
    }
}

#[test]
fn strip_re_signs_with_the_configured_key() {
    let Some(k) = key() else { return };
    for path in [DDP, JOC] {
        let data = read(path);
        for method in [Method::InPlace, Method::Repack] {
            let mut o = opts(true, true, true, method);
            o.key = Some(k.clone());
            o.allow_stale_protection = false;
            let (out, rep) = strip(&data, &o).unwrap();
            assert_eq!(rep.containers_resigned, 32, "{path} {method:?}");
            assert_crcs(&out);
            for (f, d) in frames(&out) {
                for c in emdf::containers(&f, d) {
                    assert_eq!(protect::verify(&f, d, &c, Some(&k)), Integrity::Valid);
                }
            }
        }
    }
}

#[test]
fn tampered_input_is_not_re_signed() {
    let Some(k) = key() else { return };
    let mut data = read(DDP);
    // Corrupt one primary protection byte of frame 0 and fix the CRC so the frame parses clean.
    let (p, len, old) = {
        let (f, d) = frames(&data).into_iter().next().unwrap();
        let c = emdf::containers(&f, d).remove(0);
        let p = c.frame_pos(c.protection.as_ref().unwrap().body_pos);
        (p, f.bytes, bits::get(d, p, 8))
    };
    bits::set(&mut data[..len], p, 8, old ^ 0xff);
    fix_crcs(Codec::Eac3, &mut data[..len]);
    let mut o = opts(true, true, false, Method::InPlace); // compr 0xff becomes 0
    o.key = Some(k);
    o.allow_stale_protection = false;
    assert!(matches!(strip(&data, &o), Err(Error::Protected(m)) if m.contains("already fails")));
}

#[test]
fn synthetic_dependent_substreams() {
    let data = with_dependents(&read(DDP));
    let text = show(&data, None, false).unwrap();
    assert!(
        text.contains("E-AC-3 dependent substream 0: 32 frames"),
        "{text}"
    );
    // Method A strips both substreams.
    let (out, _) = strip(&data, &opts(true, true, true, Method::InPlace)).unwrap();
    assert_crcs(&out);
    for (f, _) in frames(&out) {
        assert!(drc_values(&f).iter().all(|&(r, _, v)| match r {
            Role::Dialnorm(_) => v == 31,
            _ => v == 0,
        }));
    }
    // Method B removes dynrng from both and keeps compr, so compre still marks the last
    // dependent substream (E.2.8.5).
    for rf in [false, true] {
        let (out, _) = strip(&data, &opts(true, rf, false, Method::Repack)).unwrap();
        assert_crcs(&out);
        for (f, _) in frames(&out) {
            assert!(f.all(Role::Dynrng(0)).next().is_none());
            assert_eq!(f.value(Role::ComprExists(0), -1), Some(1));
        }
    }
    // Removing compr from a dependent substream through the API stays refused.
    let (f, _) = frames(&data).into_iter().nth(1).unwrap();
    let e = [Edit::Compr {
        program: 0,
        code: None,
    }];
    assert!(matches!(
        lower(&f, &e, Method::Repack),
        Err(Error::NotSupportedYet(m)) if m.contains("E.2.8.5")
    ));
}

#[test]
fn ac3_clause_4_5_margins_after_method_b() {
    // Constraint 1 (syncinfo, bsi, blocks 0 and 1 within 5/8) can only gain margin when bits
    // are deleted. Constraint 2 (block 5 mantissas, aux and errorcheck within the final 3/8)
    // loses margin: measure the smallest remaining margin over the AC-3 evidence streams.
    let mut min_before = usize::MAX;
    let mut min_after = usize::MAX;
    let mut n = 0;
    for (_, data) in aligned_streams() {
        let fr = frames(&data);
        if fr[0].0.codec != Codec::Ac3 {
            continue;
        }
        let (out, _) = strip(&data, &opts(true, true, false, Method::Repack)).unwrap();
        for ((of, _), (nf, _)) in fr.iter().zip(frames(&out)) {
            let five = ddpmeta::frame::ac3_five_eighths_bits(of.bytes);
            assert!(of.block_starts[2] <= five && nf.block_starts[2] <= five);
            min_before = min_before.min(of.mantissa_starts[5] - five);
            min_after = min_after.min(nf.mantissa_starts[5] - five);
            n += 1;
        }
    }
    eprintln!(
        "AC-3: {n} frames, smallest constraint 2 margin {min_before} bits before, {min_after} bits after method B"
    );
    assert!(n > 0);
}
