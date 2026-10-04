//! Audio block machinery shared by the AC-3 (4.3.3) and E-AC-3 (E.1.2.4) parsers: exponent
//! unpacking (6.1.3), bit allocation state (6.2.2), and mantissa section lengths (6.3.5 for
//! conventional mantissas, E.2.4.2 and E.2.4.4 for AHT).

use crate::alloc::{self, DeltaSeg, Params};
use crate::error::{invalid, unsupported, Result};
use crate::syntax::Cursor;
use crate::tables::{
    BAP, BAP_BITS, DB_KNEE, FAST_DECAY, FAST_GAIN, FLOOR, GAQ_BITS, HEBAP, SLOW_DECAY, SLOW_GAIN,
    VQ_BITS,
};

/// Channel slots: full bandwidth channels 0 to 4, then the coupling and LFE channels.
pub const CPL: usize = 5;
pub const LFE: usize = 6;
pub const SLOTS: usize = 7;

/// Decoded exponent set of one channel slot.
#[derive(Clone, Debug, Default)]
pub struct ExpSet {
    pub exp: Vec<i16>,
    pub start: usize,
    pub end: usize,
}

/// Read one exponent set (6.1.3). `strategy` is 1, 2 or 3 (D15, D25, D45).
pub fn read_exponents(
    cur: &mut Cursor<'_>,
    slot: usize,
    strategy: u8,
    start: usize,
    end: usize,
) -> Result<ExpSet> {
    if !(1..=3).contains(&strategy) {
        return invalid("exponent reuse where new exponents are required");
    }
    if end <= start || end > 256 {
        return invalid(format!("exponent range {start}..{end}"));
    }
    let grpsize = 1usize << (strategy - 1);
    let (absname, grpname): (&'static str, &'static str) = match slot {
        CPL => ("cplabsexp", "cplexps"),
        LFE => ("lfeexps[0]", "lfeexps"),
        _ => ("exps[ch][0]", "exps[ch]"),
    };
    let ngrps = match slot {
        CPL => (end - start) / (3 * grpsize),
        LFE => 2,
        _ => (end - 1 + 3 * grpsize - 3) / (3 * grpsize),
    };
    let index = if slot < CPL { slot } else { 0 };
    let absexp = cur.geti(absname, index, 4)? as i32;
    let mut prev = if slot == CPL { absexp << 1 } else { absexp };
    let mut exp = vec![0i16; 256];
    let mut out = Vec::with_capacity(ngrps * 3 * grpsize + 1);
    if slot != CPL {
        out.push(prev);
    }
    for _ in 0..ngrps {
        let g = cur.geti(grpname, index, 7)?;
        if g > 124 {
            return invalid(format!("grouped exponent {g} > 124"));
        }
        for m in [g / 25, (g % 25) / 5, g % 5] {
            prev += m as i32 - 2;
            if !(0..=24).contains(&prev) {
                return invalid(format!("exponent {prev} outside 0..24"));
            }
            for _ in 0..grpsize {
                out.push(prev);
            }
        }
    }
    let base = if slot == CPL { start } else { 0 };
    for (i, &e) in out.iter().enumerate() {
        if base + i < 256 {
            exp[base + i] = e as i16;
        }
    }
    if base + out.len() < end {
        return invalid("exponent set shorter than its mantissa range");
    }
    Ok(ExpSet { exp, start, end })
}

/// Bit allocation side information carried across blocks of one frame.
#[derive(Clone, Debug, Default)]
pub struct AllocState {
    /// sdcycod, fdcycod, sgaincod, dbpbcod, floorcod.
    pub bai: Option<[u8; 5]>,
    pub csnroffst: Option<u8>,
    pub fsnroffst: [u8; SLOTS],
    pub fgaincod: [u8; SLOTS],
    pub cplleak: Option<(u8, u8)>,
    /// Active delta segments by slot; empty means no delta (6.2.2.6, deltbae 2).
    pub delta: [Vec<DeltaSeg>; SLOTS],
}

impl AllocState {
    pub fn params(&self, fscod: usize) -> Result<Params> {
        let Some(b) = self.bai else {
            return invalid("bit allocation parameters used before transmission");
        };
        Ok(Params {
            sdecay: i32::from(SLOW_DECAY[b[0] as usize]),
            fdecay: i32::from(FAST_DECAY[b[1] as usize]),
            sgain: i32::from(SLOW_GAIN[b[2] as usize]),
            dbknee: i32::from(DB_KNEE[b[3] as usize]),
            floor: i32::from(FLOOR[b[4] as usize]),
            fscod,
        })
    }

    /// Read delta segments for one slot (deltnseg, then offset, length, ba per segment).
    pub fn read_delta(&mut self, cur: &mut Cursor<'_>, slot: usize) -> Result<()> {
        let (n, o, l, b): (&'static str, &'static str, &'static str, &'static str) = if slot == CPL
        {
            ("cpldeltnseg", "cpldeltoffst", "cpldeltlen", "cpldeltba")
        } else {
            ("deltnseg", "deltoffst", "deltlen", "deltba")
        };
        let nseg = cur.geti(n, slot, 3)? as usize;
        let mut segs = Vec::with_capacity(nseg + 1);
        for _ in 0..=nseg {
            let offset = cur.geti(o, slot, 5)? as u8;
            let len = cur.geti(l, slot, 4)? as u8;
            let ba = cur.geti(b, slot, 3)? as u8;
            segs.push(DeltaSeg { offset, len, ba });
        }
        self.delta[slot] = segs;
        Ok(())
    }

    /// Apply a deltbae code (Table 4.11): 0 reuse, 1 new info follows (read by the caller
    /// later), 2 no delta, 3 reserved.
    pub fn delta_code(&mut self, slot: usize, code: u32, block0: bool) -> Result<()> {
        match code {
            0 if block0 => invalid("deltbae reuse in block 0"),
            0 | 1 => Ok(()),
            2 => {
                self.delta[slot].clear();
                Ok(())
            }
            _ => invalid("reserved deltbae"),
        }
    }
}

/// One slot's allocation request for a block.
pub struct SlotAlloc<'a> {
    pub slot: usize,
    pub exps: &'a ExpSet,
    pub start: usize,
    pub end: usize,
    pub high_efficiency: bool,
}

/// Compute bap (or hebap) arrays for the given slots. Implements the all-zero SNR offset
/// special case of 6.2.2.1 over the offsets in use by this block.
pub fn allocate_block(
    st: &AllocState,
    fscod: usize,
    slots: &[SlotAlloc<'_>],
) -> Result<[Vec<u8>; SLOTS]> {
    let mut out: [Vec<u8>; SLOTS] = Default::default();
    let Some(csnr) = st.csnroffst else {
        return invalid("SNR offsets used before transmission");
    };
    let all_zero = csnr == 0 && slots.iter().all(|s| st.fsnroffst[s.slot] == 0);
    let p = st.params(fscod)?;
    for s in slots {
        if all_zero {
            out[s.slot] = vec![0; s.end];
            continue;
        }
        if s.exps.start > s.start || s.exps.end < s.end {
            return invalid("reused exponents do not cover the current mantissa range");
        }
        let leak = if s.slot == CPL {
            let Some((f, l)) = st.cplleak else {
                return invalid("coupling leak used before transmission");
            };
            Some(((i32::from(f) << 8) + 768, (i32::from(l) << 8) + 768))
        } else {
            None
        };
        let snroffset = (((i32::from(csnr) - 15) << 4) + i32::from(st.fsnroffst[s.slot])) << 2;
        let table = if s.high_efficiency { &HEBAP } else { &BAP };
        out[s.slot] = alloc::allocate(
            &alloc::Channel {
                exp: &s.exps.exp,
                start: s.start,
                end: s.end,
                fgain: i32::from(FAST_GAIN[st.fgaincod[s.slot] as usize]),
                snroffset,
                leak,
                delta: if s.slot == LFE {
                    &[]
                } else {
                    &st.delta[s.slot]
                },
            },
            &p,
            table,
        );
    }
    Ok(out)
}

/// Grouping state for bap 1, 2 and 4 within one block (6.3.5).
#[derive(Default)]
pub struct Groups {
    g1: u8,
    g2: u8,
    g4: u8,
}

/// Advance over conventional mantissas of bins `[start, end)`.
pub fn conventional(
    cur: &mut Cursor<'_>,
    bap: &[u8],
    start: usize,
    end: usize,
    g: &mut Groups,
) -> Result<()> {
    for &b in &bap[start..end] {
        let n = match b {
            0 => 0,
            1 => {
                let n = if g.g1 == 0 { 5 } else { 0 };
                g.g1 = (g.g1 + 1) % 3;
                n
            }
            2 => {
                let n = if g.g2 == 0 { 7 } else { 0 };
                g.g2 = (g.g2 + 1) % 3;
                n
            }
            4 => {
                let n = if g.g4 == 0 { 7 } else { 0 };
                g.g4 = (g.g4 + 1) % 2;
                n
            }
            b if b < 16 => BAP_BITS[b as usize],
            _ => return invalid("bap above 15"),
        };
        if n > 0 {
            cur.raw(n, "mantissa")?;
        }
    }
    Ok(())
}

/// Advance over one AHT channel's gain words and pre-mantissas for all six blocks
/// (E.1.2.4, E.2.4.2, E.2.4.4.2). Returns the number of GAQ tags seen.
pub fn aht(cur: &mut Cursor<'_>, hebap: &[u8], start: usize, end: usize) -> Result<usize> {
    let gaqmod = cur.raw(2, "gaqmod")?;
    let endbap = if gaqmod < 2 { 12 } else { 17 };
    let active: Vec<usize> = (start..end)
        .filter(|&b| hebap[b] > 7 && hebap[b] < endbap)
        .collect();
    let sections = match gaqmod {
        0 => 0,
        1 | 2 => active.len(),
        _ => active.len().div_ceil(3),
    };
    // Gains in active bin order: 1 = G1, 2 = G2, 4 = G4.
    let mut gains = Vec::with_capacity(active.len());
    for _ in 0..sections {
        match gaqmod {
            1 => gains.push(if cur.raw(1, "gaqgain")? == 1 { 2 } else { 1 }),
            2 => gains.push(if cur.raw(1, "gaqgain")? == 1 { 4 } else { 1 }),
            3 => {
                let w = cur.raw(5, "gaqgain")?;
                if w > 26 {
                    return invalid("composite GAQ gain above 26");
                }
                for m in [w / 9, (w % 9) / 3, w % 3] {
                    gains.push([1, 2, 4][m as usize]);
                }
            }
            _ => {}
        }
    }
    let mut tags = 0;
    let mut next_active = 0;
    for bin in start..end {
        let h = hebap[bin] as usize;
        if h == 0 {
            continue;
        }
        if h < 8 {
            cur.raw(VQ_BITS[h], "vector index")?;
            continue;
        }
        if h > 19 {
            return invalid("hebap above 19");
        }
        let m = GAQ_BITS[h - 8];
        let gain = if hebap[bin] < endbap && gaqmod != 0 {
            let g = gains.get(next_active).copied().unwrap_or(1);
            next_active += 1;
            g
        } else {
            1
        };
        for _ in 0..6 {
            match gain {
                1 => {
                    cur.raw(m, "gaq mantissa")?;
                }
                2 | 4 => {
                    let small = if gain == 2 { m - 1 } else { m - 2 };
                    let large = if gain == 2 { m - 1 } else { m };
                    let v = cur.raw(small, "gaq small mantissa")?;
                    if v == 1 << (small - 1) {
                        cur.raw(large, "gaq large mantissa")?;
                        tags += 1;
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(tags)
}

/// Guard for syntax the parser deliberately does not model.
pub fn refuse<T>(what: &str) -> Result<T> {
    unsupported(what.to_string())
}
