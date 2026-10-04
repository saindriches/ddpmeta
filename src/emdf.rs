//! EMDF container detection, ETSI TS 102 366 V1.4.1 Annex H (H.2.1 syntax, H.2.2 semantics).
//! Containers are found in skip fields (skipfld) and in auxiliary user data, the two carriers
//! H.1 names. Each starts with emdf_sync: syncword 0x5838 and a 16 bit byte length.
//! Only the container framing and the protection fields are decoded; payload bytes are skipped.

use crate::bits;
use crate::frame::Frame;
use crate::syntax::Role;

pub const SYNCWORD: u32 = 0x5838;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    pub id: u64,
    pub bytes: u64,
    /// Bit offset of the first payload byte within `Container::body`.
    pub body_pos: usize,
}

/// Payload type names of Table H.2.3; IDs beyond it are not defined in TS 102 366 V1.4.1.
pub fn payload_name(id: u64) -> &'static str {
    match id {
        1 => "programme loudness data (H.3.1)",
        2 => "programme information (H.3.2)",
        3 => "E-AC-3 substream structure (H.3.3)",
        4 => "dynamic range compression data for portable devices (H.3.4)",
        5 => "programme language (H.3.5)",
        6 => "external data (H.3.6)",
        7 => "headphone rendering data (H.3.7)",
        _ => "not defined in TS 102 366 V1.4.1",
    }
}

/// The leading fields of a programme loudness payload (H.3.1.2) that tie it to dialnorm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loudness {
    pub loudpractyp: u8,
    pub loudcorrdialgat: Option<bool>,
    /// loudcorrtyp (H.3.1.3), present with loudcorrdialgat when loudpractyp is not 0.
    pub loudcorrtyp: Option<bool>,
    /// loudrelgat code: -58 LUFS plus 0.5 LU per step (H.3.1.3.8).
    pub loudrelgat: Option<u8>,
    /// loudspchgat code, same scale (H.3.1.3.10).
    pub loudspchgat: Option<u8>,
    /// Bit offset of the loudrelgat code within `Container::body`, when present.
    pub loudrelgat_pos: Option<usize>,
    /// Bit offset of the loudspchgat code within `Container::body`, when present.
    pub loudspchgat_pos: Option<usize>,
}

/// Width in bits of the loudrelgat and loudspchgat codes (H.3.1.3.8, H.3.1.3.10).
pub const GATED_LOUDNESS_BITS: usize = 7;

impl Loudness {
    /// The loudness value H.3.1.3.8 or H.3.1.3.10 requires dialnorm to match, in LUFS.
    pub fn dialnorm_reference(&self) -> Option<f64> {
        let lufs = |c: u8| -58.0 + 0.5 * f64::from(c);
        match self.loudcorrdialgat {
            Some(false) => self.loudrelgat.map(lufs),
            Some(true) => self.loudspchgat.map(lufs),
            None => None,
        }
    }

    /// Body bit offset of the gated loudness code that dialnorm must match, the same one
    /// `dialnorm_reference` reads (loudrelgat when loudcorrdialgat is 0, loudspchgat when 1).
    pub fn dialnorm_reference_pos(&self) -> Option<usize> {
        match self.loudcorrdialgat {
            Some(false) => self.loudrelgat_pos,
            Some(true) => self.loudspchgat_pos,
            None => None,
        }
    }
}

/// Decode the leading loudness fields of a payload with ID 1 of container `c`.
pub fn loudness(c: &Container, p: &Payload) -> Option<Loudness> {
    if p.id != 1 {
        return None;
    }
    let mut r = Rd {
        d: &c.body,
        pos: p.body_pos,
        end: p.body_pos + 8 * usize::try_from(p.bytes).ok()?,
    };
    if r.get(2)? == 3 {
        r.get(4)?;
    }
    if r.get(1)? == 1 {
        r.get(3)?;
    }
    let loudpractyp = r.get(4)? as u8;
    let (loudcorrdialgat, loudcorrtyp) = if loudpractyp != 0 {
        let g = r.get(1)? == 1;
        let t = r.get(1)? == 1;
        (Some(g), Some(t))
    } else {
        (None, None)
    };
    let (loudrelgat, loudrelgat_pos) = if r.get(1)? == 1 {
        let pos = r.pos;
        (Some(r.get(7)? as u8), Some(pos))
    } else {
        (None, None)
    };
    let (loudspchgat, loudspchgat_pos) = if r.get(1)? == 1 {
        let pos = r.pos;
        (Some(r.get(7)? as u8), Some(pos))
    } else {
        (None, None)
    };
    Some(Loudness {
        loudpractyp,
        loudcorrdialgat,
        loudcorrtyp,
        loudrelgat,
        loudspchgat,
        loudrelgat_pos,
        loudspchgat_pos,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Protection {
    /// Length in bits of protection_bits_primary (8, 32 or 128) and secondary (0, 8, 32, 128).
    pub primary_bits: usize,
    pub secondary_bits: usize,
    /// Bit offset of protection_bits_primary within `Container::body`.
    pub body_pos: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Carrier {
    /// skipfld of the given audio blocks. One block, or several when the container continues
    /// from one skip field into the next ones (see `containers`).
    Skip(Vec<usize>),
    /// auxbits user data.
    Aux,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub carrier: Carrier,
    /// Bit position of the EMDF syncword within the syncframe.
    pub pos: usize,
    pub length_bytes: usize,
    pub version: u64,
    pub key_id: u64,
    pub payloads: Vec<Payload>,
    pub protection: Option<Protection>,
    /// Set when the container could not be decoded to its protection fields.
    pub error: Option<String>,
    /// The container's bytes after emdf_sync (`length_bytes` of them, fewer when the carrier
    /// ends first), gathered across its carrier segments. Payload and protection positions
    /// index this.
    pub body: Vec<u8>,
    /// Syncframe bit ranges `(pos, width)` holding the container from its syncword on, in
    /// order. One range unless the container continues across skip fields.
    pub spans: Vec<(usize, usize)>,
}

impl Container {
    pub fn protected(&self) -> bool {
        self.protection.is_some()
    }

    /// True when the container continues across more than one skip field.
    pub fn spanning(&self) -> bool {
        self.spans.len() > 1
    }

    /// Syncframe bit position of bit `off` of the body (the bit after emdf_sync is 0).
    pub fn frame_pos(&self, off: usize) -> usize {
        let mut v = 32 + off;
        for &(p, w) in &self.spans {
            if v < w {
                return p + v;
            }
            v -= w;
        }
        panic!("body bit {off} lies outside the container");
    }

    /// Write `n` (at most 32) bits of `value` at body offset `off` into the syncframe.
    pub fn write(&self, data: &mut [u8], off: usize, n: usize, value: u32) {
        for i in 0..n {
            let bit = (value >> (n - 1 - i)) & 1;
            bits::set(data, self.frame_pos(off + i), 1, bit);
        }
    }
}

struct Rd<'a> {
    d: &'a [u8],
    pos: usize,
    end: usize,
}

impl Rd<'_> {
    fn get(&mut self, n: usize) -> Option<u64> {
        if self.pos + n > self.end {
            return None;
        }
        let mut v = 0u64;
        let mut left = n;
        while left > 0 {
            let k = left.min(32);
            v = (v << k) | u64::from(bits::get(self.d, self.pos, k));
            self.pos += k;
            left -= k;
        }
        Some(v)
    }

    /// variable_bits (H.2.1.2.1).
    fn var(&mut self, n: usize) -> Option<u64> {
        let mut value = 0u64;
        loop {
            value = value.checked_add(self.get(n)?)?;
            if self.get(1)? == 0 {
                return Some(value);
            }
            value = (value << n).checked_add(1 << n)?;
        }
    }
}

/// Decode the container fields from its gathered body (H.2.1.1).
fn parse_container(d: &[u8], c: &mut Container) -> Option<()> {
    let mut r = Rd {
        d,
        pos: 0,
        end: 8 * c.length_bytes,
    };
    let mut version = r.get(2)?;
    if version == 3 {
        version += r.var(2)?;
    }
    c.version = version;
    let mut key_id = r.get(3)?;
    if key_id == 7 {
        key_id += r.var(3)?;
    }
    c.key_id = key_id;
    loop {
        let mut id = r.get(5)?;
        if id == 0 {
            break;
        }
        if id == 0x1F {
            id += r.var(5)?;
        }
        // emdf_payload_config (H.2.1.3)
        let smploffste = r.get(1)? == 1;
        if smploffste {
            r.get(11)?;
            r.get(1)?;
        }
        if r.get(1)? == 1 {
            r.var(11)?;
        }
        if r.get(1)? == 1 {
            r.var(2)?;
        }
        if r.get(1)? == 1 {
            r.get(8)?;
        }
        let discard = r.get(1)? == 1;
        if !discard {
            let mut aligned = false;
            if !smploffste {
                aligned = r.get(1)? == 1;
                if aligned {
                    r.get(2)?;
                }
            }
            if smploffste || aligned {
                r.get(7)?;
            }
        }
        let size = r.var(8)?;
        let ppos = r.pos;
        r.pos = r
            .pos
            .checked_add(usize::try_from(size).ok()?.checked_mul(8)?)?;
        if r.pos > r.end {
            return None;
        }
        c.payloads.push(Payload {
            id,
            bytes: size,
            body_pos: ppos,
        });
    }
    let plp = r.get(2)?;
    let pls = r.get(2)?;
    let primary_bits = [0, 8, 32, 128][plp as usize];
    let secondary_bits = [0, 8, 32, 128][pls as usize];
    if plp == 0 {
        c.error = Some("reserved protection_length_primary 0".into());
        return Some(());
    }
    let ppos = r.pos;
    r.get(primary_bits)?;
    r.get(secondary_bits)?;
    c.protection = Some(Protection {
        primary_bits,
        secondary_bits,
        body_pos: ppos,
    });
    Some(())
}

/// One carrier segment: a skip field (with its block) or the auxiliary user data.
struct Segment {
    pos: usize,
    width: usize,
    block: Option<usize>,
}

/// Syncframe ranges covering virtual bits `[a, b)` of the concatenated segments.
fn spans(segs: &[Segment], starts: &[usize], a: usize, b: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (s, &v) in segs.iter().zip(starts) {
        let lo = a.max(v);
        let hi = b.min(v + s.width);
        if hi > lo {
            out.push((s.pos + lo - v, hi - lo));
        }
    }
    out
}

/// Scan carrier segments for consecutive EMDF containers. Each segment is scanned from its
/// start; a container whose length runs past the end of its segment continues in the next
/// segment(s), and scanning resumes after it. Blu-ray JOC streams carry such containers
/// (a container starting in the skip field of block 2 and ending in that of block 3),
/// and the Dolby Reference Player decodes them; TS 102 366 V1.4.1 H.1 does not describe a
/// container crossing carriers. Bits after the last container of a segment are not EMDF.
fn scan(d: &[u8], segs: &[Segment], out: &mut Vec<Container>) {
    let mut w = crate::bits::Writer::new();
    let mut starts = Vec::with_capacity(segs.len());
    for s in segs {
        starts.push(w.len());
        w.copy(d, s.pos, s.width);
    }
    let total = w.len();
    let v = w.into_bytes();
    let mut at = 0usize;
    for (i, s) in segs.iter().enumerate() {
        let end = starts[i] + s.width;
        if at >= end {
            continue; // covered by a container that began in an earlier segment
        }
        at = at.max(starts[i]);
        while at < end && at + 32 <= total && bits::get(&v, at, 16) == SYNCWORD {
            let length_bytes = bits::get(&v, at + 16, 16) as usize;
            let body = at + 32;
            let body_end = (body + 8 * length_bytes).min(total);
            let sp = spans(segs, &starts, at, body_end);
            let blocks: Vec<usize> = segs
                .iter()
                .zip(&starts)
                .filter(|(g, &gv)| gv < body_end && gv + g.width > at)
                .filter_map(|(g, _)| g.block)
                .collect();
            let mut c = Container {
                carrier: if s.block.is_some() {
                    Carrier::Skip(blocks)
                } else {
                    Carrier::Aux
                },
                pos: sp[0].0,
                length_bytes,
                version: 0,
                key_id: 0,
                payloads: Vec::new(),
                protection: None,
                error: None,
                body: Vec::new(),
                spans: sp,
            };
            let mut bw = crate::bits::Writer::new();
            bw.copy(&v, body, body_end - body);
            let b = bw.into_bytes();
            if body_end - body < 8 * length_bytes {
                c.error = Some("container length exceeds its carrier".into());
                c.body = b;
                out.push(c);
                return;
            }
            if parse_container(&b, &mut c).is_none() && c.error.is_none() {
                c.error = Some("container syntax ends past its length".into());
            }
            c.body = b;
            out.push(c);
            at = body_end;
        }
    }
}

/// Every EMDF container found in the frame's skip fields (read as one carrier, see `scan`) and
/// in its auxiliary user data.
pub fn containers(frame: &Frame, data: &[u8]) -> Vec<Container> {
    let mut out = Vec::new();
    let skips: Vec<Segment> = frame
        .all(Role::Skipfld)
        .map(|f| Segment {
            pos: f.pos,
            width: f.width,
            block: Some(f.block as usize),
        })
        .collect();
    scan(data, &skips, &mut out);
    if frame.value(Role::AuxDataE, -1) == Some(1) {
        if let (Some(aux), Some(l)) = (
            frame.find(Role::AuxBits, -1),
            frame.value(Role::AuxDataL, -1),
        ) {
            let l = l as usize;
            let seg = Segment {
                pos: aux.end() - l,
                width: l,
                block: None,
            };
            scan(data, &[seg], &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_bits_matches_table_h_2_2() {
        // n = 2: value 4 (= 2^2) is coded as group 00 with read_more 1, then 00 and 0.
        let d = [0b0010_0000u8];
        let mut r = Rd {
            d: &d,
            pos: 0,
            end: 8,
        };
        assert_eq!(r.var(2), Some(4));
        let d = [0b1100_0000u8];
        let mut r = Rd {
            d: &d,
            pos: 0,
            end: 8,
        };
        assert_eq!(r.var(2), Some(3));
    }
}
