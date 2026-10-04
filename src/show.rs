//! `show`: a summary per substream (dialnorm, statistics of the line mode and RF mode DRC gains,
//! the EMDF containers with their protection status and loudness values). Verbose output first
//! lists every syncframe and container.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};

use crate::edit::{effective_dynrng, Op};
use crate::emdf::{self, Carrier, Container, Payload};
use crate::emdf_protection::KeySet;
use crate::error::{Error, Result};
use crate::frame::{crc_status, Codec, Frame};
use crate::gain::{compr_db, dialnorm_db, dynrng_db};
use crate::protect;
use crate::stream::{parse_frame, FrameReader};
use crate::syntax::Role;

/// Samples per audio block (4.4.3, E.1.3.1.6).
const BLOCK_SAMPLES: u64 = 256;

/// Time weighted distribution of one gain word over a substream: audio blocks per code.
struct Gains {
    blocks: [u64; 256],
    /// Gain words actually present in the stream.
    words: u64,
    /// Blocks of frames that carry no word (compr only).
    absent: u64,
}

impl Default for Gains {
    fn default() -> Self {
        Gains {
            blocks: [0; 256],
            words: 0,
            absent: 0,
        }
    }
}

/// Histogram bin labels; `bin` places a gain. Cut bins include their lower edge, boost bins their
/// upper edge, and unity has a bin of its own. dynrng spans -24 to +24 dB (6.7.2.2), compr -48
/// to +48 dB (6.7.3.2).
const BINS: [&str; 10] = [
    "below -12 dB",
    "-12 to -6 dB",
    "-6 to -3 dB",
    "-3 to -1 dB",
    "-1 to 0 dB",
    "0 dB",
    "0 to +1 dB",
    "+1 to +3 dB",
    "+3 to +6 dB",
    "above +6 dB",
];

fn bin(db: f64) -> usize {
    match db {
        d if d < -12.0 => 0,
        d if d < -6.0 => 1,
        d if d < -3.0 => 2,
        d if d < -1.0 => 3,
        d if d < 0.0 => 4,
        d if d <= 0.0 => 5,
        d if d <= 1.0 => 6,
        d if d <= 3.0 => 7,
        d if d <= 6.0 => 8,
        _ => 9,
    }
}

const BAR: usize = 30;

impl Gains {
    /// Statistics lines for one gain word: shares of time cut by more than 1 dB, within 1 dB of
    /// unity and boosted by more than 1 dB (so the near-unity codes written without
    /// compression, such as compr 0xff at -0.28 dB, do not count as a cut), the gain quantiles,
    /// and a histogram of the non-empty bins.
    fn report(&self, o: &mut String, name: &str, db: fn(u32) -> f64) {
        let total: u64 = self.blocks.iter().sum();
        let mut sorted: Vec<(f64, u64)> = (0..256u32)
            .filter(|&c| self.blocks[c as usize] > 0)
            .map(|c| (db(c), self.blocks[c as usize]))
            .collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let absent = if self.absent > 0 {
            format!(
                "; absent in {:.1}% of the time (RF mode follows dynrng there)",
                pct(self.absent, total + self.absent)
            )
        } else {
            String::new()
        };
        if self.words == 0 {
            let _ = writeln!(
                o,
                "  {name}: absent{}",
                if name.starts_with("dynrng") {
                    ", 0 dB throughout"
                } else {
                    "; RF mode follows dynrng"
                }
            );
            return;
        }
        if sorted.len() == 1 && sorted[0].0 == 0.0 {
            let _ = writeln!(o, "  {name}: 0 dB throughout, no DRC{absent}");
            return;
        }
        let mut bins = [0u64; BINS.len()];
        for &(d, n) in &sorted {
            bins[bin(d)] += n;
        }
        let share = |r: std::ops::RangeInclusive<usize>| pct(bins[r].iter().sum(), total);
        let _ = writeln!(
            o,
            "  {name}: cut over 1 dB {:.1}%, within 1 dB {:.1}%, boost over 1 dB {:.1}% of the \
             time{absent}",
            share(0..=3),
            share(4..=6),
            share(7..=9)
        );
        let quantile = |q: f64| {
            let want = ((q * total as f64).ceil() as u64).max(1);
            let mut seen = 0;
            for &(d, n) in &sorted {
                seen += n;
                if seen >= want {
                    return d;
                }
            }
            sorted[sorted.len() - 1].0
        };
        let mean = sorted.iter().map(|(d, n)| d * *n as f64).sum::<f64>() / total as f64;
        let _ = writeln!(
            o,
            "    gain dB: min {:+.2}, p10 {:+.2}, median {:+.2}, p90 {:+.2}, max {:+.2}, mean {:+.2}",
            sorted[0].0,
            quantile(0.1),
            quantile(0.5),
            quantile(0.9),
            sorted[sorted.len() - 1].0,
            mean
        );
        for (label, &n) in BINS.iter().zip(&bins) {
            if n == 0 {
                continue;
            }
            let p = pct(n, total);
            let width = ((p / 100.0 * BAR as f64).round() as usize).max(1);
            let _ = writeln!(o, "    {label:>12}  {:<BAR$} {p:5.1}%", "#".repeat(width));
        }
    }
}

fn pct(n: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}

#[derive(Default)]
struct Summary {
    frames: usize,
    blocks: u64,
    rate: Option<u32>,
    codec: String,
    dialnorm: [BTreeMap<u32, usize>; 2],
    compr: [Gains; 2],
    dynrng: [Gains; 2],
    containers: usize,
    payloads: BTreeMap<u64, usize>,
    integrity: BTreeMap<String, usize>,
    /// H.3.1 loudness values, keyed by field (in syntax order) then text.
    loudness: BTreeMap<(u8, String), usize>,
    crc_bad: usize,
}

fn kind(f: &Frame) -> String {
    match f.codec {
        Codec::Ac3 => "AC-3".into(),
        Codec::Eac3 => format!(
            "E-AC-3 {} substream {}",
            [
                "independent",
                "dependent",
                "independent (converted AC-3)",
                "reserved"
            ][f.strmtyp as usize],
            f.substreamid
        ),
    }
}

/// Sample rate from fscod (Table 4.1; E-AC-3 reduced rates are not parsed).
fn rate(fscod: u8) -> Option<u32> {
    [48000, 44100, 32000].get(usize::from(fscod)).copied()
}

/// The H.3.1 loudness values of one payload, each with its field's rank in syntax order.
fn loudness_terms(c: &Container, p: &Payload) -> Vec<(u8, String)> {
    let Some(l) = emdf::loudness(c, p) else {
        return Vec::new();
    };
    let lufs = |r: u8| -58.0 + 0.5 * f64::from(r);
    let mut t = vec![(0, format!("loudpractyp {}", l.loudpractyp))];
    if let Some(ct) = l.loudcorrtyp {
        t.push((1, format!("loudcorrtyp {}", u8::from(ct))));
    }
    if let Some(r) = l.loudrelgat {
        t.push((2, format!("loudrelgat {} LUFS", lufs(r))));
    }
    if let Some(r) = l.loudspchgat {
        t.push((3, format!("loudspchgat {} LUFS", lufs(r))));
    }
    t
}

fn dialnorm_text(code: u32) -> String {
    if code == 0 {
        "-31 dB (reserved code 0)".into()
    } else {
        format!("{} dB", dialnorm_db(code))
    }
}

/// The report for a stream held in memory.
pub fn show(data: &[u8], keys: Option<&KeySet>, verbose: bool) -> Result<String> {
    let mut out = Vec::new();
    show_to(data, keys, verbose, &mut out)?;
    Ok(String::from_utf8(out).expect("the report is text"))
}

/// Read a stream one syncframe at a time and write the report to `out`; the verbose lines of
/// each frame are written as it is read.
pub fn show_to(
    r: impl Read,
    keys: Option<&KeySet>,
    verbose: bool,
    out: &mut impl Write,
) -> Result<()> {
    let io = |e: std::io::Error| Error::Io(e.to_string());
    let mut o = String::new();
    let mut sums: BTreeMap<(u8, u8, u8), Summary> = BTreeMap::new();
    // dialnorm of the independent substream (or AC-3 frame) the following dependent
    // substream frames belong to (E.1.3.1.1), and the comparison counts.
    let mut indep: Option<Option<u32>> = None;
    let (mut dep_frames, mut dep_differ, mut dep_orphan) = (0usize, 0usize, 0usize);
    let mut first_differ = None;
    let mut frames = FrameReader::new(r);
    while let Some(fr) = frames.next_frame()? {
        let (n, at, d) = (fr.index, fr.at, fr.data);
        let f =
            parse_frame(d).map_err(|e| Error::Invalid(format!("frame {n} at byte {at}: {e}")))?;
        let dn = f.value(Role::Dialnorm(0), -1);
        if f.codec == Codec::Eac3 && f.strmtyp == 1 {
            dep_frames += 1;
            match indep {
                None => dep_orphan += 1,
                Some(i) if i != dn => {
                    dep_differ += 1;
                    first_differ.get_or_insert((n, i, dn));
                }
                Some(_) => {}
            }
        } else {
            indep = Some(dn);
        }
        let crc = crc_status(f.codec, d);
        let crc_ok = crc.crc2 && crc.crc1 != Some(false);
        let s = sums
            .entry((f.codec as u8, f.strmtyp, f.substreamid))
            .or_default();
        s.frames += 1;
        s.blocks += f.numblks as u64;
        s.rate = rate(f.fscod);
        s.codec = kind(&f);
        if !crc_ok {
            s.crc_bad += 1;
        }
        if verbose {
            let _ = writeln!(
                o,
                "frame {n} @{at}: {} bsid {} acmod {} lfeon {} {} bytes, {} blocks, CRC {}",
                kind(&f),
                f.bsid,
                f.acmod,
                u8::from(f.lfeon),
                f.bytes,
                f.numblks,
                if crc_ok { "ok" } else { "BAD" }
            );
        }
        let keep = vec![Op::Keep; f.fields.len()];
        for p in 0..2u8 {
            let Some(dn) = f.value(Role::Dialnorm(p), -1) else {
                continue;
            };
            let sfx = if p == 0 { "" } else { "2" };
            *s.dialnorm[p as usize].entry(dn).or_default() += 1;
            let compr = f.value(Role::Compr(p), -1);
            let cg = &mut s.compr[p as usize];
            match compr {
                Some(c) => {
                    cg.words += 1;
                    cg.blocks[c as usize] += f.numblks as u64;
                }
                None => cg.absent += f.numblks as u64,
            }
            let dg = &mut s.dynrng[p as usize];
            for v in effective_dynrng(&f, &keep, p) {
                dg.blocks[v as usize] += 1;
            }
            let mut blocks = Vec::new();
            for b in 0..f.numblks {
                match f.value(Role::Dynrng(p), b as i8) {
                    Some(v) => {
                        dg.words += 1;
                        blocks.push(format!("b{b} 0x{v:02x} {:+.2} dB", dynrng_db(v)));
                    }
                    None => blocks.push(format!(
                        "b{b} {}",
                        if b == 0 { "absent (0 dB)" } else { "reuse" }
                    )),
                }
            }
            if verbose {
                let _ = writeln!(
                    o,
                    "  dialnorm{sfx} {dn} ({} dB{})  compr{sfx} {}  dynrng{sfx}: {}",
                    dialnorm_db(dn),
                    if dn == 0 { ", reserved code" } else { "" },
                    compr.map_or("absent (RF mode uses dynrng)".into(), |c| format!(
                        "0x{c:02x} {:+.2} dB",
                        compr_db(c)
                    )),
                    blocks.join(", ")
                );
            }
        }
        for c in emdf::containers(&f, d) {
            s.containers += 1;
            for p in &c.payloads {
                *s.payloads.entry(p.id).or_default() += 1;
                for term in loudness_terms(&c, p) {
                    *s.loudness.entry(term).or_default() += 1;
                }
            }
            let integrity = match &c.error {
                Some(e) => format!("undecodable: {e}"),
                None if c.protection.is_none() => "no protection".into(),
                None => protect::verify(&f, d, &c, keys).to_string(),
            };
            *s.integrity.entry(integrity.clone()).or_default() += 1;
            if verbose {
                let carrier = match &c.carrier {
                    Carrier::Skip(b) if b.len() == 1 => format!("skipfld of block {}", b[0]),
                    Carrier::Skip(b) => format!(
                        "skipfld of blocks {} (continued across skip fields, which \
                         TS 102 366 V1.4.1 H.1 does not describe)",
                        b.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Carrier::Aux => "auxdata".into(),
                };
                let ids: Vec<String> = c
                    .payloads
                    .iter()
                    .map(|p| {
                        let mut t =
                            format!("{} ({} bytes, {})", p.id, p.bytes, emdf::payload_name(p.id));
                        for (_, term) in loudness_terms(&c, p) {
                            let _ = write!(t, " {term}");
                        }
                        t
                    })
                    .collect();
                let prot = c.protection.as_ref().map_or("none".into(), |p| {
                    format!("{}+{} bits", p.primary_bits, p.secondary_bits)
                });
                let _ = writeln!(
                    o,
                    "  EMDF container at bit {} in {carrier}: {} bytes, version {}, key_id {}, payloads [{}], protection {prot}: {integrity}",
                    c.pos, c.length_bytes, c.version, c.key_id, ids.join("; ")
                );
            }
        }
        out.write_all(o.as_bytes()).map_err(io)?;
        o.clear();
    }
    if verbose {
        let _ = writeln!(o);
    }
    for s in sums.values() {
        let secs = s
            .rate
            .map(|r| {
                format!(
                    ", {:.2} s",
                    (s.blocks * BLOCK_SAMPLES) as f64 / f64::from(r)
                )
            })
            .unwrap_or_default();
        let _ = writeln!(
            o,
            "{}: {} frames{secs}{}",
            s.codec,
            s.frames,
            if s.crc_bad > 0 {
                format!(", {} with BAD CRC", s.crc_bad)
            } else {
                String::new()
            }
        );
        for p in 0..2 {
            if s.dialnorm[p].is_empty() {
                continue;
            }
            let sfx = if p == 0 { "" } else { "2" };
            let dn = if s.dialnorm[p].len() == 1 {
                let (c, _) = s.dialnorm[p].iter().next().expect("one entry");
                format!("{} in every frame", dialnorm_text(*c))
            } else {
                s.dialnorm[p]
                    .iter()
                    .map(|(c, k)| format!("{} x{k}", dialnorm_text(*c)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(o, "  dialnorm{sfx}: {dn}");
            s.dynrng[p].report(&mut o, &format!("dynrng{sfx} (line mode)"), dynrng_db);
            s.compr[p].report(&mut o, &format!("compr{sfx} (RF mode)"), compr_db);
        }
        if s.containers > 0 {
            let ids: Vec<String> = s
                .payloads
                .iter()
                .map(|(i, k)| format!("{i} x{k}"))
                .collect();
            let st: Vec<String> = s
                .integrity
                .iter()
                .map(|(i, k)| format!("{i} x{k}"))
                .collect();
            let _ = writeln!(
                o,
                "  EMDF: {} containers, payload IDs [{}], protection: {}",
                s.containers,
                ids.join(", "),
                st.join("; ")
            );
            if !s.loudness.is_empty() {
                let l: Vec<String> = s
                    .loudness
                    .iter()
                    .map(|((_, t), k)| format!("{t} x{k}"))
                    .collect();
                let _ = writeln!(o, "  loudness (H.3.1): {}", l.join(", "));
            }
        } else {
            let _ = writeln!(o, "  EMDF: none");
        }
    }
    if dep_frames > 0 {
        let code = |v: Option<u32>| v.map_or("absent".into(), |c| c.to_string());
        let detail = match first_differ {
            Some((n, i, dn)) => format!(
                "differs in {dep_differ} of them (first at frame {n}: dialnorm {} against {} in \
                 its independent substream)",
                code(dn),
                code(i)
            ),
            None => "equals that of its independent substream in all of them".into(),
        };
        let _ = writeln!(
            o,
            "programme dialnorm: {dep_frames} dependent substream frames; dialnorm {detail}{}",
            if dep_orphan > 0 {
                format!("; {dep_orphan} frames precede any independent substream frame")
            } else {
                String::new()
            }
        );
    }
    out.write_all(o.as_bytes()).map_err(io)?;
    out.flush().map_err(io)
}
