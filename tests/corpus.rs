//! The streams in tests/data: AC-3, DD+, DD+ JOC and the Blu-ray profile (an AC-3 independent
//! substream with an E-AC-3 dependent substream). Covers CRCs of both substreams, EMDF containers
//! continuing across skip fields, the protection rule for dependent substreams, key_id handling,
//! dialnorm consistency across the substreams of a programme, and the `show` summary.

mod common;

use common::{data, frames, key};
use ddpmeta::edit::{Method, Strip};
use ddpmeta::emdf;
use ddpmeta::emdf_protection::KeySet;
use ddpmeta::error::Error;
use ddpmeta::frame::{crc_status, Codec};
use ddpmeta::protect::{self, Integrity};
use ddpmeta::show::show;
use ddpmeta::strip::{strip, Options};
use ddpmeta::syntax::Role;

const STREAMS: [&str; 9] = [
    "ddp51.ec3",
    "dd51.ac3",
    "bd71.ec3",
    "ddp51drc.ec3",
    "dd51drc.ac3",
    "bd71drc.ec3",
    "joc.ec3",
    "jocbd.ec3",
    "joc_bluray.ec3",
];

/// AC-3 independent substream with an E-AC-3 dependent substream.
const BLU_RAY: [&str; 4] = ["bd71.ec3", "bd71drc.ec3", "jocbd.ec3", "joc_bluray.ec3"];

fn opts(line: bool, rf: bool, dialnorm: bool, method: Method) -> Options {
    Options {
        strip: Strip { line, rf, dialnorm },
        method,
        key: None,
        allow_stale_protection: true,
    }
}

/// The H.3.1 loudness payload records the loudness mode in band, independent of the
/// protection key_id: measure_only streams (key_id 1) carry loudcorrtyp 1, measure_and_correct
/// streams (key_id 0) loudcorrtyp 0. Checked on `ddp51.ec3` (measure_and_correct) and
/// `bd71.ec3` (measure_only).
#[test]
fn loudcorrtyp_tracks_the_loudness_mode_alongside_key_id() {
    for (name, measure_only) in [("ddp51.ec3", false), ("bd71.ec3", true)] {
        let d = data(name);
        let mut seen = 0;
        for (f, fr) in frames(&d) {
            for c in emdf::containers(&f, fr) {
                for p in &c.payloads {
                    if let Some(ct) = emdf::loudness(&c, p).and_then(|l| l.loudcorrtyp) {
                        assert_eq!(ct, measure_only, "{name}: loudcorrtyp");
                        assert_eq!(c.key_id, u64::from(measure_only), "{name}: key_id");
                        seen += 1;
                    }
                }
            }
        }
        assert!(seen > 0, "{name}: no loudness payload carried loudcorrtyp");
    }
}

/// Every frame passes its CRC checks: crc1 and crc2 for AC-3, crc2 for E-AC-3.
fn assert_crcs(name: &str, data: &[u8]) {
    for (i, (f, d)) in frames(data).into_iter().enumerate() {
        let c = crc_status(f.codec, d);
        assert!(c.crc2, "{name} frame {i}: {c:?}");
        if f.codec == Codec::Ac3 {
            assert_eq!(c.crc1, Some(true), "{name} frame {i}");
        }
    }
}

#[test]
fn every_stream_parses_with_valid_crcs() {
    for name in STREAMS {
        assert_crcs(name, &data(name));
    }
}

#[test]
fn blu_ray_streams_alternate_ac3_and_dependent_e_ac3() {
    for name in BLU_RAY {
        let d = data(name);
        let fr = frames(&d);
        assert!(!fr.is_empty() && fr.len().is_multiple_of(2), "{name}");
        for (i, (f, _)) in fr.iter().enumerate() {
            if i.is_multiple_of(2) {
                assert_eq!(f.codec, Codec::Ac3, "{name} frame {i}");
            } else {
                assert_eq!((f.codec, f.strmtyp), (Codec::Eac3, 1), "{name} frame {i}");
                // compre marks the last (here the only) dependent substream (E.2.8.5).
                assert_eq!(f.value(Role::ComprExists(0), -1), Some(1), "{name}");
            }
        }
        let text = show(&d, None, false).unwrap();
        let n = fr.len() / 2;
        assert!(
            text.contains(&format!(
                "programme dialnorm: {n} dependent substream frames; dialnorm equals that of \
                 its independent substream in all of them"
            )),
            "{name}: {text}"
        );
    }
}

#[test]
fn strip_reaches_both_substreams_with_both_methods() {
    for name in [
        "bd71drc.ec3",
        "jocbd.ec3",
        "joc_bluray.ec3",
        "dd51drc.ac3",
        "ddp51drc.ec3",
    ] {
        let d = data(name);
        for method in [Method::InPlace, Method::Repack] {
            let (out, rep) = strip(&d, &opts(true, true, true, method))
                .unwrap_or_else(|e| panic!("{name} {method:?}: {e}"));
            assert_eq!(out.len(), d.len());
            assert_crcs(name, &out);
            for (f, _) in frames(&out) {
                assert!(f.all(Role::Dynrng(0)).all(|x| x.value == 0), "{name}");
                if method == Method::Repack {
                    assert!(f.all(Role::Dynrng(0)).next().is_none(), "{name}");
                }
                assert_eq!(f.value(Role::Compr(0), -1), Some(0), "{name}");
                assert_eq!(f.value(Role::Dialnorm(0), -1), Some(31), "{name}");
            }
            let text = show(&out, None, false).unwrap();
            if BLU_RAY.contains(&name) {
                assert!(
                    text.contains("dialnorm equals that of its independent"),
                    "{name}"
                );
            }
            // Every protected container was left stale (no key in these options).
            let protected: usize = frames(&d)
                .iter()
                .map(|(f, x)| {
                    emdf::containers(f, x)
                        .iter()
                        .filter(|c| c.protected())
                        .count()
                })
                .sum();
            assert_eq!(rep.containers_stale, protected, "{name} {method:?}");
        }
    }
}

#[test]
fn containers_continuing_across_skip_fields_decode() {
    for (name, total, spanning) in [("jocbd.ec3", 16, 10), ("joc_bluray.ec3", 16, 15)] {
        let d = data(name);
        let mut n = 0;
        let mut s = 0;
        for (f, x) in frames(&d) {
            for c in emdf::containers(&f, x) {
                assert_eq!(c.error, None, "{name}");
                assert!(c.protected());
                assert_eq!(c.body.len(), c.length_bytes);
                n += 1;
                if c.spanning() {
                    s += 1;
                    assert!(matches!(&c.carrier, emdf::Carrier::Skip(b) if b.len() > 1));
                }
            }
        }
        assert_eq!((n, s), (total, spanning), "{name}");
    }
}

#[test]
fn dependent_substream_protection_verifies_with_the_key() {
    let Some(k) = key() else { return };
    for (name, total) in [("jocbd.ec3", 16), ("joc_bluray.ec3", 16), ("joc.ec3", 40)] {
        let d = data(name);
        let mut n = 0;
        for (f, x) in frames(&d) {
            for c in emdf::containers(&f, x) {
                assert_eq!(
                    protect::verify(&f, x, &c, Some(&k)),
                    Integrity::Valid,
                    "{name}"
                );
                n += 1;
            }
        }
        assert_eq!(n, total, "{name}");
    }
}

#[test]
fn strip_re_signs_dependent_substream_containers() {
    let Some(k) = key() else { return };
    for (name, total) in [("jocbd.ec3", 16), ("joc_bluray.ec3", 16)] {
        let d = data(name);
        for method in [Method::InPlace, Method::Repack] {
            let mut o = opts(true, true, false, method);
            o.key = Some(k.clone());
            o.allow_stale_protection = false;
            let (out, rep) = strip(&d, &o).unwrap();
            assert_eq!(rep.containers_resigned, total, "{name} {method:?}");
            assert_crcs(name, &out);
            for (f, x) in frames(&out) {
                for c in emdf::containers(&f, x) {
                    assert_eq!(protect::verify(&f, x, &c, Some(&k)), Integrity::Valid);
                }
            }
        }
    }
}

#[test]
fn key_id_1_containers_re_sign_with_the_key_and_are_refused_without_it() {
    // Blu-ray 7.1 with key_id 1 containers in the dependent substream, one per frame. Frames whose
    // dynrng words are all 0 already stay untouched.
    let d = data("bd71drc.ec3");
    let changed = frames(&d)
        .iter()
        .filter(|(f, _)| f.strmtyp == 1 && f.all(Role::Dynrng(0)).any(|x| x.value != 0))
        .count();
    assert_eq!(changed, 24);

    // No key: the edit changes authenticated bits, so refuse unless allowed stale.
    let mut o = opts(true, false, false, Method::InPlace);
    o.allow_stale_protection = false;
    o.key = None;
    match strip(&d, &o) {
        Err(Error::Protected(m)) => assert!(m.contains("--allow-stale-protection"), "{m}"),
        r => panic!("expected a refusal without a key, got {r:?}"),
    }
    o.allow_stale_protection = true;
    let (_, rep) = strip(&d, &o).unwrap();
    assert_eq!(rep.containers_stale, changed);
    assert_eq!(rep.containers_resigned, 0);
    assert!(rep.notes.iter().any(|n| n.contains("stale protection")));

    // The rest needs the configured keys (key_id 0 and 1).
    let Some(k) = key() else { return };

    // Only key_id 0 configured: a key_id 1 container cannot be re-signed, so refuse.
    let mut only0 = KeySet::new();
    only0.insert(0, k.get(0).expect("key_id 0").clone());
    let mut o = opts(true, false, false, Method::InPlace);
    o.allow_stale_protection = false;
    o.key = Some(only0);
    match strip(&d, &o) {
        Err(Error::Protected(m)) => {
            assert!(m.contains("no key configured for key_id 1"), "{m}");
            assert!(m.contains("--allow-stale-protection"), "{m}");
        }
        r => panic!("expected a refusal with only key_id 0, got {r:?}"),
    }

    // Both keys configured: re-sign the key_id 1 containers and verify the output.
    let mut o = opts(true, false, false, Method::InPlace);
    o.allow_stale_protection = false;
    o.key = Some(k.clone());
    let (out, rep) = strip(&d, &o).unwrap();
    assert_eq!(rep.containers_resigned, changed);
    assert_eq!(rep.containers_stale, 0);
    assert_crcs("bd71drc.ec3", &out);
    for (f, x) in frames(&out) {
        for c in emdf::containers(&f, x) {
            assert_eq!(protect::verify(&f, x, &c, Some(&k)), Integrity::Valid);
        }
    }
    let text = show(&d, Some(&k), false).unwrap();
    assert!(text.contains("primary valid, secondary valid"), "{text}");
}

#[test]
fn compr_neutral_codes_in_the_ac3_core_and_dependent_substream() {
    let d = data("bd71drc.ec3");
    let (out, rep) = strip(&d, &opts(false, true, false, Method::Repack)).unwrap();
    assert_crcs("bd71drc.ec3", &out);
    let not_ff = frames(&d)
        .iter()
        .filter(|(f, _)| f.value(Role::Compr(0), -1) != Some(0xff))
        .count();
    assert_eq!(not_ff, 48);
    assert_eq!(rep.compr_set_ff, not_ff);
    for ((of, _), (nf, _)) in frames(&d).into_iter().zip(frames(&out)) {
        assert_eq!(nf.value(Role::Compr(0), -1), Some(0xff));
        assert_eq!(
            of.all(Role::Dynrng(0)).map(|f| f.value).collect::<Vec<_>>(),
            nf.all(Role::Dynrng(0)).map(|f| f.value).collect::<Vec<_>>()
        );
    }
}

/// `show` prints only the summary unless verbose, and the verbose report ends with the same
/// summary. The DRC statistics are time weighted: the AC-3 stream carries 112 dynrng words for
/// 528 blocks and the E-AC-3 stream one per block, and with the reuse rule of 4.4.3.3 both give
/// the same line mode distribution, since both were encoded from the same source and profile.
#[test]
fn show_summary_and_drc_statistics() {
    let d = data("ddp51drc.ec3");
    let brief = show(&d, None, false).unwrap();
    let full = show(&d, None, true).unwrap();
    assert!(!brief.contains("frame 0 @"), "{brief}");
    assert_eq!(full.lines().filter(|l| l.starts_with("frame ")).count(), 88);
    assert!(full.ends_with(&brief), "{full}");
    let line = "  dynrng (line mode): cut over 1 dB 30.9%, within 1 dB 37.1%, boost over 1 dB \
                32.0% of the time\n    gain dB: min -2.50, p10 -1.97, median -0.14, p90 +3.15, \
                max +3.52, mean +0.23\n";
    assert!(brief.contains(line), "{brief}");
    let ac3 = show(&data("dd51drc.ac3"), None, false).unwrap();
    assert!(ac3.contains(line), "{ac3}");
    let (out, _) = strip(&d, &opts(true, true, false, Method::InPlace)).unwrap();
    let text = show(&out, None, false).unwrap();
    assert!(
        text.contains("  dynrng (line mode): 0 dB throughout, no DRC\n")
            && text.contains("  compr (RF mode): 0 dB throughout, no DRC\n"),
        "{text}"
    );
}
