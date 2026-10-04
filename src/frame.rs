//! A parsed syncframe and the parts shared by both syntaxes: frame recognition, the tail
//! (auxdata 4.3.4 / E.1.2.5 and errorcheck 4.3.5 / E.1.2.6) and the CRC checks (6.10.1, E.2.2).

use crate::bits;
use crate::error::{invalid, unsupported, Result};
use crate::syntax::{check_contiguous, Cursor, Field, Role};
use crate::tables::ac3_frame_words;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// AC-3, bsid 0 to 8 (bsid 6 uses the Annex D alternate BSI).
    Ac3,
    /// E-AC-3, bsid 11 to 16 (Annex E).
    Eac3,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub codec: Codec,
    pub bytes: usize,
    pub fields: Vec<Field>,
    pub bsid: u8,
    pub acmod: u8,
    pub lfeon: bool,
    pub fscod: u8,
    /// E-AC-3 strmtyp (0 independent, 1 dependent, 2 converted from AC-3); 0 for AC-3.
    pub strmtyp: u8,
    pub substreamid: u8,
    pub numblks: usize,
    /// Bit positions of each block's first element, and the end of the last block.
    pub block_starts: Vec<usize>,
    pub mantissa_starts: Vec<usize>,
    pub audio_end: usize,
    /// Number of GAQ large mantissa tags walked (E-AC-3 AHT only).
    pub aht_tags: usize,
    /// Channels coded with AHT (E-AC-3 only), as slot indices.
    pub aht_slots: Vec<usize>,
    pub spx_blocks: usize,
    pub cpl_blocks: usize,
}

impl Frame {
    pub fn total_bits(&self) -> usize {
        self.bytes * 8
    }

    pub fn find(&self, role: Role, block: i8) -> Option<&Field> {
        self.fields
            .iter()
            .find(|f| f.role == role && f.block == block)
    }

    pub fn all(&self, role: Role) -> impl Iterator<Item = &Field> {
        self.fields.iter().filter(move |f| f.role == role)
    }

    pub fn value(&self, role: Role, block: i8) -> Option<u32> {
        self.find(role, block).map(|f| f.value)
    }

    /// Element by name outside audio blocks.
    pub fn named(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Number of auxbits that are not user data (the padding a repack may grow).
    pub fn aux_padding(&self) -> usize {
        let aux = self.find(Role::AuxBits, -1).map_or(0, |f| f.width);
        let user = if self.value(Role::AuxDataE, -1) == Some(1) {
            self.value(Role::AuxDataL, -1).unwrap_or(0) as usize
        } else {
            0
        };
        aux.saturating_sub(user)
    }
}

/// Peek at the frame starting at `data[0]` and return its codec and length in bytes.
pub fn probe(data: &[u8]) -> Result<(Codec, usize)> {
    if data.len() < 6 {
        return invalid("short frame header");
    }
    if bits::get(data, 0, 16) != 0x0B77 {
        return invalid("no sync word");
    }
    let bsid = bits::get(data, 40, 5);
    match bsid {
        0..=8 => {
            let fscod = bits::get(data, 32, 2) as usize;
            let frmsizecod = bits::get(data, 34, 6) as usize;
            match ac3_frame_words(fscod, frmsizecod) {
                Some(w) => Ok((Codec::Ac3, 2 * w)),
                None => invalid(format!("AC-3 fscod {fscod} frmsizecod {frmsizecod}")),
            }
        }
        11..=16 => {
            let frmsiz = bits::get(data, 21, 11) as usize;
            Ok((Codec::Eac3, 2 * (frmsiz + 1)))
        }
        _ => unsupported(format!("bsid {bsid}")),
    }
}

/// Parse auxdata and errorcheck from the end of the last audio block to the frame end.
pub fn read_tail(cur: &mut Cursor<'_>, total: usize, codec: Codec) -> Result<()> {
    if total < cur.pos + 18 {
        return invalid(format!(
            "audio data ends at bit {}, leaving no room for auxdatae and errorcheck in {total}",
            cur.pos
        ));
    }
    let auxdatae_at = total - 18;
    let auxdatae = bits::get(cur.data, auxdatae_at, 1);
    let aux_end = if auxdatae == 1 {
        if auxdatae_at < cur.pos + 14 {
            return invalid("auxdatal overlaps audio data");
        }
        auxdatae_at - 14
    } else {
        auxdatae_at
    };
    cur.block = -1;
    cur.span("auxbits", Role::AuxBits, aux_end - cur.pos)?;
    if auxdatae == 1 {
        let auxbits = cur.fields[cur.fields.len() - 1].width;
        let l = cur.get_role("auxdatal", Role::AuxDataL, -1, 14)? as usize;
        if l > auxbits {
            return invalid(format!("auxdatal {l} exceeds {auxbits} auxbits"));
        }
    }
    cur.get_role("auxdatae", Role::AuxDataE, -1, 1)?;
    let rsv = if codec == Codec::Ac3 {
        "crcrsv"
    } else {
        "encinfo"
    };
    cur.get_role(rsv, Role::CrcRsv, -1, 1)?;
    cur.get_role("crc2", Role::Crc2, -1, 16)?;
    check_contiguous(&cur.fields, total)
}

/// AC-3 5/8 boundary in bits (6.10.1).
pub fn ac3_five_eighths_bits(frame_bytes: usize) -> usize {
    let words = frame_bytes / 2;
    ((words >> 1) + (words >> 3)) * 16
}

/// CRC status of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrcStatus {
    /// AC-3 crc1 (None for E-AC-3).
    pub crc1: Option<bool>,
    pub crc2: bool,
}

pub fn crc_status(codec: Codec, frame: &[u8]) -> CrcStatus {
    let crc2 = crate::crc::crc16(0, &frame[2..]) == 0;
    let crc1 = (codec == Codec::Ac3).then(|| {
        let five = ac3_five_eighths_bits(frame.len()) / 8;
        crate::crc::crc16(0, &frame[2..five]) == 0
    });
    CrcStatus { crc1, crc2 }
}

/// Recompute every checksum of an edited frame in place: AC-3 crc1 so that the CRC register is
/// zero at the 5/8 point, then crc2 (for both syntaxes) so that it is zero at the frame end.
/// The AC-3 crcrsv bit is inverted if crc2 would equal the sync word (4.4.5.1).
pub fn fix_crcs(codec: Codec, frame: &mut [u8]) {
    let n = frame.len();
    if codec == Codec::Ac3 {
        let five = ac3_five_eighths_bits(n) / 8;
        frame[2] = 0;
        frame[3] = 0;
        // CRC is linear: crc(c || rest) = crc(c || 0...) ^ crc(0 || rest). Solve for c.
        let target = crate::crc::crc16(0, &frame[2..five]);
        let mut cols = [0u16; 16];
        let mut probe = vec![0u8; five - 2];
        for (bit, col) in cols.iter_mut().enumerate() {
            let v = 1u16 << (15 - bit);
            probe[0] = (v >> 8) as u8;
            probe[1] = v as u8;
            *col = crate::crc::crc16(0, &probe);
        }
        let c = solve(&cols, target);
        frame[2] = (c >> 8) as u8;
        frame[3] = c as u8;
    }
    for attempt in 0..2 {
        let c = crate::crc::crc16(0, &frame[2..n - 2]);
        if codec == Codec::Ac3 && c == 0x0B77 && attempt == 0 {
            // Invert crcrsv, the bit immediately before crc2.
            frame[n - 3] ^= 1;
            continue;
        }
        frame[n - 2] = (c >> 8) as u8;
        frame[n - 1] = c as u8;
        break;
    }
}

/// Solve sum_i x_i * cols[i] = target over GF(2), where x_i is bit (15 - i) of x.
fn solve(cols: &[u16; 16], target: u16) -> u16 {
    // Gaussian elimination on the 16 x 16 system.
    let mut rows: Vec<(u16, u16)> = Vec::new(); // (combination of columns, value)
    let mut basis: Vec<(u16, u16)> = Vec::new();
    for (i, &c) in cols.iter().enumerate() {
        rows.push((c, 1u16 << (15 - i)));
    }
    for (mut v, mut comb) in rows {
        for &(bv, bc) in &basis {
            if v ^ bv < v {
                v ^= bv;
                comb ^= bc;
            }
        }
        if v != 0 {
            basis.push((v, comb));
            basis.sort_by_key(|b| std::cmp::Reverse(b.0));
        }
    }
    let mut t = target;
    let mut x = 0;
    for &(bv, bc) in &basis {
        if t ^ bv < t {
            t ^= bv;
            x ^= bc;
        }
    }
    debug_assert_eq!(t, 0, "CRC system is invertible");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc1_solver_zeroes_register_at_five_eighths() {
        let mut frame = vec![0u8; 512];
        frame[0] = 0x0b;
        frame[1] = 0x77;
        for (i, b) in frame.iter_mut().enumerate().skip(4) {
            *b = (i * 37 % 251) as u8;
        }
        fix_crcs(Codec::Ac3, &mut frame);
        let s = crc_status(Codec::Ac3, &frame);
        assert_eq!(
            s,
            CrcStatus {
                crc1: Some(true),
                crc2: true
            }
        );
    }
}
