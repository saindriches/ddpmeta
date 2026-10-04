//! Parametric bit allocation, written from ETSI TS 102 366 V1.4.1 clauses 6.2.2.1 to 6.2.2.7
//! and E.2.4.3.1 (the hebap variant). Only the resulting bap/hebap arrays are needed here:
//! they give the mantissa lengths that locate everything after block 0.
//!
//! calc_lowcomp follows the printed `(b0 + 256) == b1` test. A `>=` test would agree on every
//! valid stream because lowcomp is only evaluated below band 20, where every band is one bin, so
//! adjacent band PSDs differ by a multiple of 128 bounded by the differential exponent range of 2
//! (6.1.2), at most 256.

use crate::tables::{band_start, masktab, BAND_END, HEARING, LOG_ADD};

/// Decoded bit allocation parameters in the 6.2.2.1 table units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub sdecay: i32,
    pub fdecay: i32,
    pub sgain: i32,
    pub dbknee: i32,
    pub floor: i32,
    pub fscod: usize,
}

/// One delta bit allocation segment (6.2.2.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeltaSeg {
    pub offset: u8,
    pub len: u8,
    pub ba: u8,
}

/// Inputs of one exponent set.
pub struct Channel<'a> {
    /// Absolute exponents indexed by bin; only `[start, end)` is read.
    pub exp: &'a [i16],
    pub start: usize,
    pub end: usize,
    pub fgain: i32,
    pub snroffset: i32,
    /// Coupling channel leak initialization (fastleak, slowleak).
    pub leak: Option<(i32, i32)>,
    pub delta: &'a [DeltaSeg],
}

fn logadd(a: i32, b: i32) -> i32 {
    let c = a - b;
    let address = ((c.abs() >> 1).min(255)) as usize;
    if c >= 0 {
        a + LOG_ADD[address]
    } else {
        b + LOG_ADD[address]
    }
}

fn calc_lowcomp(a: i32, b0: i32, b1: i32, bin: usize) -> i32 {
    if bin < 7 {
        if b0 + 256 == b1 {
            384
        } else if b0 > b1 {
            (a - 64).max(0)
        } else {
            a
        }
    } else if bin < 20 {
        if b0 + 256 == b1 {
            320
        } else if b0 > b1 {
            (a - 64).max(0)
        } else {
            a
        }
    } else {
        (a - 128).max(0)
    }
}

/// Bit allocation pointers for bins `[start, end)` (zero elsewhere), using `table`
/// (`tables::BAP` for 6.2.2.7 or `tables::HEBAP` for E.2.4.3.1).
pub fn allocate(c: &Channel<'_>, p: &Params, table: &[u8; 64]) -> Vec<u8> {
    let (start, end) = (c.start, c.end);
    let mut out = vec![0u8; end.max(start)];
    if end <= start {
        return out;
    }
    // 6.2.2.2
    let mut psd = vec![0i32; end];
    for bin in start..end {
        psd[bin] = 3072 - (i32::from(c.exp[bin]) << 7);
    }
    // 6.2.2.3
    let mut bndpsd = [0i32; 50];
    let mut j = start;
    let mut k = masktab(start);
    loop {
        let lastbin = (band_start(k) + (BAND_END[k] as usize - band_start(k))).min(end);
        bndpsd[k] = psd[j];
        j += 1;
        while j < lastbin {
            bndpsd[k] = logadd(bndpsd[k], psd[j]);
            j += 1;
        }
        k += 1;
        if end <= lastbin {
            break;
        }
    }
    // 6.2.2.4
    let bndstrt = masktab(start);
    let bndend = masktab(end - 1) + 1;
    let mut excite = [0i32; 50];
    let (mut fastleak, mut slowleak) = c.leak.unwrap_or((0, 0));
    let begin = if bndstrt == 0 {
        let lfe_last = |bin: usize| bndend == 7 && bin == 6;
        let mut lowcomp = 0;
        lowcomp = calc_lowcomp(lowcomp, bndpsd[0], bndpsd[1], 0);
        excite[0] = bndpsd[0] - c.fgain - lowcomp;
        lowcomp = calc_lowcomp(lowcomp, bndpsd[1], bndpsd[2], 1);
        excite[1] = bndpsd[1] - c.fgain - lowcomp;
        let mut b = 7;
        for bin in 2..7 {
            if !lfe_last(bin) {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak = bndpsd[bin] - c.fgain;
            slowleak = bndpsd[bin] - p.sgain;
            excite[bin] = fastleak - lowcomp;
            if !lfe_last(bin) && bndpsd[bin] <= bndpsd[bin + 1] {
                b = bin + 1;
                break;
            }
        }
        for bin in b..bndend.min(22) {
            if !lfe_last(bin) {
                lowcomp = calc_lowcomp(lowcomp, bndpsd[bin], bndpsd[bin + 1], bin);
            }
            fastleak -= p.fdecay;
            fastleak = fastleak.max(bndpsd[bin] - c.fgain);
            slowleak -= p.sdecay;
            slowleak = slowleak.max(bndpsd[bin] - p.sgain);
            excite[bin] = (fastleak - lowcomp).max(slowleak);
        }
        22
    } else {
        bndstrt
    };
    for bin in begin..bndend {
        fastleak -= p.fdecay;
        fastleak = fastleak.max(bndpsd[bin] - c.fgain);
        slowleak -= p.sdecay;
        slowleak = slowleak.max(bndpsd[bin] - p.sgain);
        excite[bin] = fastleak.max(slowleak);
    }
    // 6.2.2.5
    let mut mask = [0i32; 50];
    for bin in bndstrt..bndend {
        if bndpsd[bin] < p.dbknee {
            excite[bin] += (p.dbknee - bndpsd[bin]) >> 2;
        }
        mask[bin] = excite[bin].max(i32::from(HEARING[50 * p.fscod + bin]));
    }
    // 6.2.2.6
    let mut band = 0usize;
    for seg in c.delta {
        band += usize::from(seg.offset);
        let delta = if seg.ba >= 4 {
            (i32::from(seg.ba) - 3) << 7
        } else {
            (i32::from(seg.ba) - 4) << 7
        };
        for _ in 0..seg.len {
            if band < 50 {
                mask[band] += delta;
            }
            band += 1;
        }
    }
    // 6.2.2.7
    let mut i = start;
    let mut j = masktab(start);
    loop {
        let lastbin = (BAND_END[j] as usize).min(end);
        let mut m = mask[j] - c.snroffset - p.floor;
        if m < 0 {
            m = 0;
        }
        m &= 0x1fe0;
        m += p.floor;
        while i < lastbin {
            let address = ((psd[i] - m) >> 5).clamp(0, 63) as usize;
            out[i] = table[address];
            i += 1;
        }
        j += 1;
        if end <= lastbin {
            break;
        }
    }
    out
}
