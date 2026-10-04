//! Metadata edits as a general plan. An `Edit` names what should change; `lower` turns a list
//! of edits for one syncframe into either bit patches (method A, no length change of any
//! element) or a repack (method B, gain words deleted and the freed bits absorbed as zero
//! padding at the start of auxbits). `apply` produces the edited frame and `validate` re-parses
//! it and checks that nothing but the planned elements changed.
//!
//! Only the strip subset is implemented: dialnorm to code 31; dynrng to code 0 (method A) or
//! removed (method B); compr to code 0x00 or 0xff, or removed through the API only. Every other
//! edit returns `Error::NotSupportedYet`.
//!
//! The strip command never removes compr, under either method. Both Dolby decoders measured play
//! a syncframe without compr in RF mode like line mode, about 11 dB below RF mode with compr
//! present, so removing it changes the RF level. They also read compr 0x00 as "use dynrng" in RF
//! mode: with dynrng left in place, compr 0x00 makes RF mode follow dynrng. So `--rf` writes 0x00 only when `--line` zeroes or removes
//! dynrng as well, and otherwise 0xff (-0.28 dB, TS 102 366 V1.4.1 6.7.3.2), the code streams
//! without RF compression carry (every Blu-ray JOC frame of tests/data/joc_bluray.ec3).

use crate::bits::{self, Writer};
use crate::error::{Error, Result};
use crate::frame::{ac3_five_eighths_bits, crc_status, Codec, Frame};
use crate::stream::parse_frame;
use crate::syntax::Role;

/// One metadata change. `program` is 0 for the first programme and 1 for the second
/// programme of 1+1 mode (acmod 0: dialnorm2, compr2, dynrng2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Set dialnorm (programme 0) or dialnorm2 (programme 1).
    Dialnorm { program: u8, code: u8 },
    /// Set compr or compr2 to a code, or remove the word (`None`).
    Compr { program: u8, code: Option<u8> },
    /// Set dynrng or dynrng2 of one block (`Some`) or every block (`None`) to a code, or remove
    /// the words (`code: None`).
    Dynrng {
        program: u8,
        block: Option<u8>,
        code: Option<u8>,
    },
    /// Any other element by name.
    Field { name: String, value: u32 },
}

/// How gain words are neutralized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Method A: write 0 dB codes into the existing words. No element moves.
    InPlace,
    /// Method B: clear the presence flags, delete the words, pad auxbits.
    Repack,
}

/// The strip operations the command line exposes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Strip {
    /// Line mode DRC: dynrng and dynrng2.
    pub line: bool,
    /// RF mode DRC: compr and compr2.
    pub rf: bool,
    /// dialnorm and dialnorm2 to code 31 (-31 dB).
    pub dialnorm: bool,
}

/// compr code written by `--rf` together with `--line`: 0 dB, and "use dynrng" in the Dolby
/// decoders, where dynrng is then 0 dB too.
pub const COMPR_WITH_LINE: u8 = 0x00;
/// compr code written by `--rf` alone: -0.28 dB in both Dolby decoders.
pub const COMPR_ALONE: u8 = 0xff;

impl Strip {
    pub fn edits(&self, method: Method) -> Vec<Edit> {
        let code = match method {
            Method::InPlace => Some(0),
            Method::Repack => None,
        };
        let compr = if self.line {
            COMPR_WITH_LINE
        } else {
            COMPR_ALONE
        };
        let mut out = Vec::new();
        for program in 0..2 {
            if self.line {
                out.push(Edit::Dynrng {
                    program,
                    block: None,
                    code,
                });
            }
            if self.rf {
                out.push(Edit::Compr {
                    program,
                    code: Some(compr),
                });
            }
            if self.dialnorm {
                out.push(Edit::Dialnorm { program, code: 31 });
            }
        }
        out
    }
}

/// What to do with one parsed element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Keep,
    Set(u32),
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitPatch {
    pub pos: usize,
    pub width: usize,
    pub value: u32,
}

/// A lowered plan for one syncframe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lowered {
    /// Method A, or method B where no word needed deleting.
    Patches(Vec<BitPatch>),
    /// Method B: one op per parsed element.
    Repack(Vec<Op>),
}

impl Lowered {
    pub fn is_empty(&self) -> bool {
        match self {
            Lowered::Patches(p) => p.is_empty(),
            Lowered::Repack(ops) => ops.iter().all(|o| *o == Op::Keep),
        }
    }

    /// Bits freed by deleted words.
    pub fn freed_bits(&self, frame: &Frame) -> usize {
        match self {
            Lowered::Patches(_) => 0,
            Lowered::Repack(ops) => frame
                .fields
                .iter()
                .zip(ops)
                .filter(|(_, o)| **o == Op::Drop)
                .map(|(f, _)| f.width)
                .sum(),
        }
    }
}

fn not_yet<T>(s: impl Into<String>) -> Result<T> {
    Err(Error::NotSupportedYet(s.into()))
}

/// Lower edits for one syncframe.
pub fn lower(frame: &Frame, edits: &[Edit], method: Method) -> Result<Lowered> {
    let mut ops = vec![Op::Keep; frame.fields.len()];
    let mut rf_programs = Vec::new();
    for e in edits {
        match e {
            Edit::Field { name, .. } => return not_yet(format!("editing {name}")),
            Edit::Dialnorm { program, code } => {
                check_program(*program)?;
                if *code != 31 {
                    return not_yet("setting dialnorm to a code other than 31 (-31 dB)");
                }
                for (i, f) in frame.fields.iter().enumerate() {
                    if f.role == Role::Dialnorm(*program) {
                        ops[i] = Op::Set(31);
                    }
                }
            }
            Edit::Dynrng {
                program,
                block,
                code,
            } => {
                check_program(*program)?;
                if block.is_some() {
                    return not_yet("editing dynrng of a single block");
                }
                if matches!(code, Some(c) if *c != 0) {
                    return not_yet("setting dynrng to a code other than 0 (0 dB)");
                }
                gain_words(
                    frame,
                    &mut ops,
                    Role::Dynrng(*program),
                    Role::DynrngExists(*program),
                    *code,
                    method,
                )?;
            }
            Edit::Compr { program, code } => {
                check_program(*program)?;
                if matches!(code, Some(c) if *c != COMPR_WITH_LINE && *c != COMPR_ALONE) {
                    return not_yet("setting compr to a code other than 0x00 or 0xff");
                }
                if code.is_none()
                    && *program == 0
                    && frame.codec == Codec::Eac3
                    && frame.strmtyp == 1
                    && frame.value(Role::ComprExists(0), -1) == Some(1)
                {
                    return not_yet(
                        "removing compr from a dependent substream: compre marks the last \
                         dependent substream of a programme (E.2.8.5), so clearing it changes \
                         which substream's DRC words apply; use method A",
                    );
                }
                gain_words(
                    frame,
                    &mut ops,
                    Role::Compr(*program),
                    Role::ComprExists(*program),
                    *code,
                    method,
                )?;
                rf_programs.push(*program);
            }
        }
    }
    // RF mode result: where no compr word remains (the source has none, or the API removed
    // it), RF mode decoders use dynrng (6.7.3.1), so the frame only has 0 dB RF gain if every
    // effective dynrng value is 0.
    for &p in &rf_programs {
        if !has_program(frame, p) {
            continue;
        }
        let compr_left = frame
            .fields
            .iter()
            .zip(&ops)
            .any(|(f, o)| f.role == Role::Compr(p) && *o != Op::Drop);
        if !compr_left && effective_dynrng(frame, &ops, p).iter().any(|&v| v != 0) {
            return not_yet(format!(
                "after the edit this syncframe has no {} word, so RF mode decoders apply \
                 {} instead (6.7.3.1), which is not 0 dB here; inserting a compr word is not \
                 supported yet, strip line mode as well",
                if p == 0 { "compr" } else { "compr2" },
                if p == 0 { "dynrng" } else { "dynrng2" }
            ));
        }
    }
    if method == Method::Repack && ops.contains(&Op::Drop) {
        return Ok(Lowered::Repack(ops));
    }
    let patches = frame
        .fields
        .iter()
        .zip(&ops)
        .filter_map(|(f, o)| match o {
            Op::Set(v) if *v != f.value => Some(BitPatch {
                pos: f.pos,
                width: f.width,
                value: *v,
            }),
            _ => None,
        })
        .collect();
    Ok(Lowered::Patches(patches))
}

fn check_program(p: u8) -> Result<()> {
    if p > 1 {
        return Err(Error::Invalid(format!(
            "programme {p} does not exist (0 or 1)"
        )));
    }
    Ok(())
}

fn has_program(frame: &Frame, p: u8) -> bool {
    frame.fields.iter().any(|f| f.role == Role::Dialnorm(p))
}

fn gain_words(
    frame: &Frame,
    ops: &mut [Op],
    word: Role,
    flag: Role,
    code: Option<u8>,
    method: Method,
) -> Result<()> {
    match (code, method) {
        (Some(c), _) => {
            for (i, f) in frame.fields.iter().enumerate() {
                if f.role == word {
                    ops[i] = Op::Set(u32::from(c));
                }
            }
            Ok(())
        }
        (None, Method::InPlace) => {
            not_yet("removing gain words under method A; method B (repack) removes them")
        }
        (None, Method::Repack) => {
            for i in 0..frame.fields.len() {
                if frame.fields[i].role == word {
                    let ok = i > 0
                        && frame.fields[i - 1].role == flag
                        && frame.fields[i - 1].block == frame.fields[i].block;
                    if !ok {
                        return Err(Error::Invalid(format!(
                            "{} at bit {} is not preceded by its presence flag",
                            frame.fields[i].name, frame.fields[i].pos
                        )));
                    }
                    ops[i] = Op::Drop;
                    ops[i - 1] = Op::Set(0);
                }
            }
            Ok(())
        }
    }
}

/// dynrng value of every block after `ops`, with the reuse rule of 4.4.3.3: a missing word
/// reuses the previous block's value, except block 0 where it is 0.
pub fn effective_dynrng(frame: &Frame, ops: &[Op], program: u8) -> Vec<u32> {
    let mut out = Vec::with_capacity(frame.numblks);
    let mut cur = 0;
    for blk in 0..frame.numblks {
        let word = frame
            .fields
            .iter()
            .zip(ops)
            .find(|(f, _)| f.role == Role::Dynrng(program) && f.block == blk as i8);
        cur = match word {
            Some((_, Op::Drop)) | None if blk == 0 => 0,
            Some((_, Op::Drop)) | None => cur,
            Some((_, Op::Set(v))) => *v,
            Some((f, Op::Keep)) => f.value,
        };
        out.push(cur);
    }
    out
}

/// Produce the edited syncframe. Checksums are left stale; the caller fixes them after any
/// EMDF re-signing.
pub fn apply(frame: &Frame, data: &[u8], plan: &Lowered) -> Result<Vec<u8>> {
    match plan {
        Lowered::Patches(p) => {
            let mut out = data[..frame.bytes].to_vec();
            for b in p {
                bits::set(&mut out, b.pos, b.width, b.value);
            }
            Ok(out)
        }
        Lowered::Repack(ops) => {
            if frame.value(Role::BlkStrtInfoE, -1) == Some(1) {
                return Err(Error::Unsupported(
                    "method B with blkstrtinfo present: it describes block start positions \
                     and its encoding is not specified (E.1.3.2.27)"
                        .into(),
                ));
            }
            let freed = plan.freed_bits(frame);
            let mut w = Writer::new();
            for (f, op) in frame.fields.iter().zip(ops) {
                match op {
                    Op::Drop => {}
                    Op::Set(v) => w.put(f.width, *v),
                    Op::Keep if f.role == Role::AuxBits => {
                        w.zeros(freed);
                        w.copy(data, f.pos, f.width);
                    }
                    Op::Keep => w.copy(data, f.pos, f.width),
                }
            }
            debug_assert_eq!(w.len(), frame.total_bits());
            Ok(w.into_bytes())
        }
    }
}

/// Bit ranges `(pos, len)` of the edited frame whose content may differ from the source for
/// reasons other than the plan: EMDF protection values that were re-signed.
pub type Mask = Vec<(usize, usize)>;

/// Span bits with the relative ranges `rel` zeroed.
fn masked(data: &[u8], pos: usize, width: usize, rel: &Mask) -> Vec<u8> {
    let mut w = Writer::new();
    w.copy(data, pos, width);
    let mut b = w.into_bytes();
    for &(rp, rl) in rel {
        for p in rp..rp + rl {
            bits::set(&mut b, p, 1, 0);
        }
    }
    b
}

/// Re-parse the edited frame and check it against the source: same size, valid checksums,
/// every element identical except the planned ones (method B: the deleted words are gone, the
/// flags are 0 and auxbits grew by exactly the freed bits at its start), and the AC-3
/// constraints of clause 4.5.
pub fn validate(orig: &Frame, od: &[u8], new: &[u8], plan: &Lowered, mask: &Mask) -> Result<Frame> {
    let fail = |s: String| Err(Error::Invalid(format!("edited frame check failed: {s}")));
    if new.len() != orig.bytes {
        return fail(format!("size {} instead of {}", new.len(), orig.bytes));
    }
    let nf = parse_frame(new)?;
    let c = crc_status(nf.codec, new);
    if !c.crc2 || c.crc1 == Some(false) {
        return fail(format!("checksums {c:?}"));
    }
    let (ops, freed) = match plan {
        Lowered::Patches(p) => {
            let mut ops = vec![Op::Keep; orig.fields.len()];
            for b in p {
                if let Some(i) = orig
                    .fields
                    .iter()
                    .position(|f| f.pos == b.pos && f.width == b.width)
                {
                    ops[i] = Op::Set(b.value);
                }
            }
            (ops, 0)
        }
        Lowered::Repack(ops) => (ops.clone(), plan.freed_bits(orig)),
    };
    let kept = ops.iter().filter(|o| **o != Op::Drop).count();
    if kept != nf.fields.len() {
        return fail(format!("{} elements instead of {kept}", nf.fields.len()));
    }
    let mut shift = 0usize; // bits deleted before the current element
    let mut nfi = nf.fields.iter();
    for (of, op) in orig.fields.iter().zip(&ops) {
        if *op == Op::Drop {
            shift += of.width;
            continue;
        }
        let f = nfi.next().expect("counted");
        let grow = if of.role == Role::AuxBits { freed } else { 0 };
        if f.name != of.name || f.role != of.role || f.block != of.block || f.index != of.index {
            return fail(format!(
                "element {} at bit {} became {}",
                of.name, of.pos, f.name
            ));
        }
        if f.pos + shift != of.pos || f.width != of.width + grow {
            return fail(format!(
                "element {} at bit {} moved or resized",
                of.name, of.pos
            ));
        }
        // auxbits absorbs the freed bits: elements after it keep their source positions.
        shift -= grow;
        if of.role.is_checksum() || of.role == Role::CrcRsv {
            continue;
        }
        if of.role.is_span() || of.width > 32 {
            let at = f.pos + grow;
            let rel: Mask = mask
                .iter()
                .filter_map(|&(p, l)| {
                    let s = p.max(at);
                    let e = (p + l).min(at + of.width);
                    (e > s).then(|| (s - at, e - s))
                })
                .collect();
            if masked(od, of.pos, of.width, &rel) != masked(new, at, of.width, &rel)
                || !bits::range_zero(new, f.pos, grow)
            {
                return fail(format!("content of {} at bit {} changed", of.name, of.pos));
            }
            continue;
        }
        let want = match op {
            Op::Set(v) => *v,
            _ => of.value,
        };
        if f.value != want {
            return fail(format!(
                "{} at bit {} is {} instead of {want}",
                of.name, of.pos, f.value
            ));
        }
    }
    if nf.codec == Codec::Ac3 && nf.numblks == 6 {
        let five = ac3_five_eighths_bits(nf.bytes);
        let c1 = |fr: &Frame| fr.block_starts[2] <= five;
        let c2 = |fr: &Frame| fr.mantissa_starts[5] >= five;
        if c1(orig) && !c1(&nf) {
            return fail("blocks 0 and 1 no longer end within 5/8 of the syncframe (4.5)".into());
        }
        if c2(orig) && !c2(&nf) {
            return Err(Error::NotSupportedYet(
                "method B would move block 5 mantissa data before the 5/8 point, violating \
                 clause 4.5 constraint 2; use method A"
                    .into(),
            ));
        }
    }
    Ok(nf)
}
