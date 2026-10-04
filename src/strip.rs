//! Stream level strip. For every syncframe of every substream: lower and apply the plan, keep
//! EMDF protection valid (re-sign with a configured key) or refuse, fix the checksums and
//! validate the result. Frames are processed one at a time, so memory does not grow with the
//! stream.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::edit::{apply, lower, validate, Lowered, Mask, Method, Strip, COMPR_ALONE};
use crate::emdf::{self, Container};
use crate::emdf_protection::KeySet;
use crate::error::{Error, Result};
use crate::frame::fix_crcs;
use crate::protect::{self, Integrity};
use crate::stream::{parse_frame, FrameReader};
use crate::syntax::Role;

pub struct Options {
    pub strip: Strip,
    pub method: Method,
    pub key: Option<KeySet>,
    /// Write frames whose EMDF protection no longer matches instead of refusing.
    pub allow_stale_protection: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub frames: usize,
    pub frames_changed: usize,
    pub words_zeroed: usize,
    pub words_removed: usize,
    pub dialnorm_set: usize,
    /// Programme loudness references set to -31 LUFS to stay consistent with the stripped
    /// dialnorm (H.3.1.3.8, H.3.1.3.10).
    pub loudness_set: usize,
    pub freed_bits: usize,
    pub containers_resigned: usize,
    pub containers_stale: usize,
    /// compr words set to 0xff (`edit::COMPR_ALONE`, -0.28 dB); not counted in `words_zeroed`.
    pub compr_set_ff: usize,
    pub notes: Vec<String>,
}

/// What a container with stale protection does to playback, as measured.
pub const STALE_EFFECT: &str = "measured: the Dolby Reference Player 4.3 stops decoding when \
     a container in a substream it decodes fails primary verification (DD+ and DD+ JOC, \
     channels and objects; Blu-ray 7.1 output; Blu-ray JOC objects, while its 5.1 core still \
     decodes), a wrong secondary value alone had no effect, and dee_ddp_decoder output did \
     not change";

/// Containers whose protection an edit may invalidate: every protected container, and every
/// container that could not be decoded far enough to tell.
fn protected(cs: &[Container]) -> bool {
    cs.iter()
        .any(|c| c.protection.is_some() || c.error.is_some())
}

/// True when every protected container still authenticates the same core and body.
fn protection_unaffected(
    of: &crate::frame::Frame,
    od: &[u8],
    nf: &crate::frame::Frame,
    nd: &[u8],
) -> bool {
    let (Some(a), Some(b)) = (
        protect::authenticated(of, od),
        protect::authenticated(nf, nd),
    ) else {
        return false;
    };
    if a != b {
        return false;
    }
    let oc = emdf::containers(of, od);
    let nc = emdf::containers(nf, nd);
    oc.len() == nc.len()
        && oc
            .iter()
            .zip(&nc)
            .all(|(x, y)| x.length_bytes == y.length_bytes && x.body == y.body)
}

/// Strips one syncframe at a time; the report accumulates over the frames.
pub struct Stripper<'a> {
    opt: &'a Options,
    edits: Vec<crate::edit::Edit>,
    rep: Report,
}

impl<'a> Stripper<'a> {
    pub fn new(opt: &'a Options) -> Self {
        Stripper {
            opt,
            edits: opt.strip.edits(opt.method),
            rep: Report::default(),
        }
    }

    /// The edited syncframe `n` found at byte `at`, or `None` when the plan leaves it
    /// unchanged. Neither method changes a frame's size: method A writes words in place and
    /// method B pads auxbits with the bits it frees.
    pub fn frame(&mut self, n: usize, at: u64, od: &[u8]) -> Result<Option<Vec<u8>>> {
        let opt = self.opt;
        let ctx = |e: Error| match e {
            Error::Truncated(s) => Error::Truncated(format!("frame {n} at byte {at}: {s}")),
            Error::Unsupported(s) => Error::Unsupported(format!("frame {n} at byte {at}: {s}")),
            Error::Invalid(s) => Error::Invalid(format!("frame {n} at byte {at}: {s}")),
            Error::NotSupportedYet(s) => {
                Error::NotSupportedYet(format!("frame {n} at byte {at}: {s}"))
            }
            Error::Protected(s) => Error::Protected(format!("frame {n} at byte {at}: {s}")),
            Error::Io(s) => Error::Io(s),
        };
        let rep = &mut self.rep;
        rep.frames += 1;
        let of = parse_frame(od).map_err(ctx)?;
        let plan = lower(&of, &self.edits, opt.method).map_err(ctx)?;
        if plan.is_empty() {
            return Ok(None);
        }
        let mut nd = apply(&of, od, &plan).map_err(ctx)?;
        let nf = parse_frame(&nd).map_err(ctx)?;
        let ocs = emdf::containers(&of, od);
        // Bits of the edited frame that differ from the source for reasons other than the plan:
        // re-signed protection, and the programme loudness references edited just below.
        let mut mask: Mask = Vec::new();

        // Keep dialnorm and the programme loudness payload consistent (H.3.1.3.8, H.3.1.3.10).
        // The --dialnorm strip sets dialnorm to -31 dB (its reference level), so the gated
        // loudness the payload ties to dialnorm (loudrelgat or loudspchgat) is set to -31 LUFS
        // to match. The code is a fixed 7 bit field, so this is an in place edit; it is inside
        // the EMDF container body, which the protection authenticates, so the container is
        // re-signed below. Without a key the edit is refused there, like any other authenticated
        // change.
        if opt.strip.dialnorm {
            const TARGET_LUFS: f64 = -31.0;
            let code = crate::gain::loudness_code(TARGET_LUFS);
            for c in emdf::containers(&nf, &nd) {
                for p in &c.payloads {
                    let Some(l) = emdf::loudness(&c, p) else {
                        continue;
                    };
                    let (Some(lufs), Some(off)) =
                        (l.dialnorm_reference(), l.dialnorm_reference_pos())
                    else {
                        continue;
                    };
                    if (lufs - TARGET_LUFS).abs() <= 0.5 {
                        continue; // already at the reference level
                    }
                    c.write(&mut nd, off, emdf::GATED_LOUDNESS_BITS, u32::from(code));
                    for i in 0..emdf::GATED_LOUDNESS_BITS {
                        mask.push((c.frame_pos(off + i), 1));
                    }
                    rep.loudness_set += 1;
                }
            }
        }

        if (opt.strip.line || opt.strip.rf)
            && ocs.iter().any(|c| c.payloads.iter().any(|p| p.id == 4))
            && !rep.notes.iter().any(|s| s.starts_with("portable"))
        {
            rep.notes.push(
                "portable device DRC payloads (EMDF payload ID 4, H.3.4) are left in place; \
             editing them is not supported yet"
                    .into(),
            );
        }

        // Both Dolby decoders measured read compr 0x00 as "use dynrng" in RF mode, so a line strip
        // that keeps such a compr word also changes RF mode.
        if opt.strip.line
            && !opt.strip.rf
            && of.value(Role::Compr(0), -1) == Some(0)
            && !rep.notes.iter().any(|s| s.starts_with("compr 0x00"))
        {
            rep.notes.push(
                "compr 0x00 is present; the Dolby decoders measured then follow dynrng in RF \
             mode, so this line strip changes RF mode as well"
                    .into(),
            );
        }

        // EMDF protection
        if protected(&ocs) && !protection_unaffected(&of, od, &nf, &nd) {
            let unsignable = opt.key.as_ref().and_then(|k| {
                ocs.iter()
                    .filter(|c| c.protection.is_some() || c.error.is_some())
                    .find_map(|c| {
                        protect::expected(&of, od, c, k)
                            .err()
                            .map(|why| format!("container at bit {}: {why}", c.pos))
                    })
            });
            match (&opt.key, unsignable) {
                (Some(key), None) => {
                    for c in ocs.iter().filter(|c| c.protection.is_some()) {
                        let v = protect::verify(&of, od, c, Some(key));
                        if v != Integrity::Valid {
                            if opt.allow_stale_protection {
                                rep.containers_stale += 1;
                                continue;
                            }
                            return Err(ctx(Error::Protected(format!(
                                "the input container at bit {} already fails verification \
                             ({v}); refusing to re-sign it",
                                c.pos
                            ))));
                        }
                    }
                    let k =
                        protect::resign(&nf, &mut nd, key).map_err(|s| ctx(Error::Protected(s)))?;
                    rep.containers_resigned += k;
                    for c in emdf::containers(&nf, &nd) {
                        if let Some(p) = &c.protection {
                            for i in 0..p.primary_bits + p.secondary_bits {
                                mask.push((c.frame_pos(p.body_pos + i), 1));
                            }
                        }
                    }
                }
                _ if opt.allow_stale_protection => {
                    rep.containers_stale += ocs
                        .iter()
                        .filter(|c| c.protection.is_some() || c.error.is_some())
                        .count();
                }
                (None, _) => {
                    return Err(ctx(Error::Protected(format!(
                        "the edit changes bits that the EMDF protection authenticates and no \
                     key is configured (--emdf-key-file or EMDF_PROTECTION_KEY_FILE); \
                     pass --allow-stale-protection to write the containers with stale \
                     protection; {STALE_EFFECT}"
                    ))));
                }
                (Some(_), Some(why)) => {
                    return Err(ctx(Error::Protected(format!(
                        "the edit changes bits that the EMDF protection authenticates and the \
                     containers cannot be re-signed ({why}); pass \
                     --allow-stale-protection to write them with stale protection; \
                     {STALE_EFFECT}"
                    ))));
                }
            }
        }

        fix_crcs(nf.codec, &mut nd);
        let checked = validate(&of, od, &nd, &plan, &mask).map_err(ctx)?;
        if let Some(key) = &opt.key {
            if !mask.is_empty() {
                for c in emdf::containers(&checked, &nd) {
                    let v = protect::verify(&checked, &nd, &c, Some(key));
                    if c.protection.is_some() && v != Integrity::Valid {
                        return Err(ctx(Error::Protected(format!(
                            "re-signed container does not verify ({v})"
                        ))));
                    }
                }
            }
        }
        rep.frames_changed += 1;
        rep.freed_bits += plan.freed_bits(&of);
        match &plan {
            Lowered::Patches(p) => {
                for b in p {
                    match of.fields.iter().find(|f| f.pos == b.pos).map(|f| f.role) {
                        Some(Role::Dialnorm(_)) => rep.dialnorm_set += 1,
                        Some(Role::Compr(_)) if b.value == u32::from(COMPR_ALONE) => {
                            rep.compr_set_ff += 1
                        }
                        _ => rep.words_zeroed += 1,
                    }
                }
            }
            Lowered::Repack(ops) => {
                for (f, o) in of.fields.iter().zip(ops) {
                    match (f.role, o) {
                        (_, crate::edit::Op::Drop) => rep.words_removed += 1,
                        (Role::Dialnorm(_), crate::edit::Op::Set(v)) if *v != f.value => {
                            rep.dialnorm_set += 1
                        }
                        (Role::Compr(_), crate::edit::Op::Set(v))
                            if *v != f.value && *v == u32::from(COMPR_ALONE) =>
                        {
                            rep.compr_set_ff += 1
                        }
                        (Role::Compr(_) | Role::Dynrng(_), crate::edit::Op::Set(v))
                            if *v != f.value =>
                        {
                            rep.words_zeroed += 1
                        }
                        _ => {}
                    }
                }
            }
        }
        if nd.len() != od.len() {
            return Err(ctx(Error::Invalid(format!(
                "edited frame of {} bytes, {} expected",
                nd.len(),
                od.len()
            ))));
        }
        Ok(Some(nd))
    }

    /// The report, with the notes that summarize the whole stream.
    pub fn finish(mut self) -> Report {
        if self.rep.compr_set_ff > 0 {
            self.rep.notes.push(format!(
                "{} compr words set to 0xff (-0.28 dB), not 0x00: the Dolby decoders measured treat \
                 compr 0x00 as \"use dynrng\" in RF mode, and dynrng is kept",
                self.rep.compr_set_ff
            ));
        }
        if self.rep.containers_stale > 0 {
            self.rep.notes.push(format!(
                "{} EMDF containers left with stale protection; {STALE_EFFECT}",
                self.rep.containers_stale
            ));
        }
        self.rep
    }
}

/// Strip a whole stream held in memory.
pub fn strip(data: &[u8], opt: &Options) -> Result<(Vec<u8>, Report)> {
    let mut out = Vec::with_capacity(data.len());
    let rep = strip_stream(data, &mut out, opt)?;
    Ok((out, rep))
}

/// Strip from a reader to a writer, one syncframe at a time. On an error the writer holds the
/// frames before the failing one.
pub fn strip_stream(r: impl Read, w: &mut impl Write, opt: &Options) -> Result<Report> {
    let mut frames = FrameReader::new(r);
    let mut s = Stripper::new(opt);
    while let Some(f) = frames.next_frame()? {
        let edited = s.frame(f.index, f.at, f.data)?;
        w.write_all(edited.as_deref().unwrap_or(f.data))
            .map_err(io)?;
    }
    w.flush().map_err(io)?;
    Ok(s.finish())
}

/// Strip a file in place, with method A only: method B rewrites most of every frame, so it
/// saves nothing over writing a new file. One pass edits every syncframe and records only the
/// bytes that change (a few percent of a stream at most); they are written once every frame has
/// succeeded, so a refused stream is left untouched. An I/O error or an interruption while
/// writing leaves the file partly edited.
pub fn strip_in_place(path: &Path, opt: &Options) -> Result<Report> {
    if opt.method != Method::InPlace {
        return Err(Error::NotSupportedYet(
            "editing a file in place with method B; it rewrites most of every frame".into(),
        ));
    }
    let mut s = Stripper::new(opt);
    let mut changes = Changes::default();
    let mut frames = FrameReader::new(BufReader::new(File::open(path).map_err(io)?));
    while let Some(f) = frames.next_frame()? {
        if let Some(nd) = s.frame(f.index, f.at, f.data)? {
            changes.record(f.at, f.data, &nd);
        }
    }
    if !changes.buf.is_empty() {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(io)?;
        changes.apply(&mut file).map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    Ok(s.finish())
}

/// The bytes an in-place strip changes, in file order. Each run is stored as two LEB128
/// numbers, the gap from the end of the previous run and the run length, then its bytes, so it
/// costs about two bytes more than the bytes it changes.
#[derive(Default)]
struct Changes {
    buf: Vec<u8>,
    /// File offset where the last recorded run ends.
    end: u64,
}

/// Unchanged bytes up to this many between two changed ones are written along with them,
/// which is cheaper than starting a new run.
const MERGE_GAP: usize = 2;

/// Bytes read, patched and written back at once.
const WINDOW: usize = 1 << 20;

impl Changes {
    fn record(&mut self, at: u64, old: &[u8], new: &[u8]) {
        let mut i = 0;
        while i < old.len() {
            if old[i] == new[i] {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i + 1;
            let mut j = end;
            while j < old.len() && j <= end + MERGE_GAP {
                if old[j] != new[j] {
                    end = j + 1;
                }
                j += 1;
            }
            let pos = at + start as u64;
            leb128(&mut self.buf, pos - self.end);
            leb128(&mut self.buf, (end - start) as u64);
            self.buf.extend_from_slice(&new[start..end]);
            self.end = pos + (end - start) as u64;
            i = end;
        }
    }

    /// Write the runs into `file`, a window of the file at a time.
    fn apply(&self, file: &mut File) -> std::io::Result<()> {
        let len = file.metadata()?.len();
        let mut window: Vec<u8> = Vec::new();
        let mut win_at = 0u64;
        let (mut rest, mut pos) = (self.buf.as_slice(), 0u64);
        while !rest.is_empty() {
            pos += read_leb128(&mut rest);
            let n = read_leb128(&mut rest) as usize;
            let (bytes, tail) = rest.split_at(n);
            rest = tail;
            let inside = pos >= win_at && pos + n as u64 <= win_at + window.len() as u64;
            if !inside {
                write_window(file, win_at, &window)?;
                win_at = pos;
                let size = (len - pos).min(WINDOW.max(n) as u64) as usize;
                window.resize(size, 0);
                file.seek(SeekFrom::Start(pos))?;
                file.read_exact(&mut window)?;
            }
            let off = (pos - win_at) as usize;
            window[off..off + n].copy_from_slice(bytes);
            pos += n as u64;
        }
        write_window(file, win_at, &window)
    }
}

fn write_window(file: &mut File, at: u64, window: &[u8]) -> std::io::Result<()> {
    if !window.is_empty() {
        file.seek(SeekFrom::Start(at))?;
        file.write_all(window)?;
    }
    Ok(())
}

fn leb128(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn read_leb128(b: &mut &[u8]) -> u64 {
    let (mut v, mut shift) = (0u64, 0);
    loop {
        let byte = b[0];
        *b = &b[1..];
        v |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return v;
        }
        shift += 7;
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}
