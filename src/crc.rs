//! CRC-16 used by AC-3 and E-AC-3.
//!
//! The polynomial is x^16 + x^15 + x^2 + 1, represented as 0x8005. Bytes are processed
//! most significant bit first, with initial value zero and no final XOR. E-AC-3 places the
//! checksum in the last two bytes of the frame and covers the bytes after the sync word.

const POLY: u16 = 0x8005;

const fn table() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ POLY
            } else {
                c << 1
            };
            bit += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TABLE: [u16; 256] = table();

pub fn crc16(init: u16, bytes: &[u8]) -> u16 {
    let mut c = init;
    for &b in bytes {
        c = (c << 8) ^ TABLE[((c >> 8) as u8 ^ b) as usize];
    }
    c
}

pub fn frame_crc_ok(frame: &[u8]) -> bool {
    frame.len() >= 4 && crc16(0, &frame[2..]) == 0
}

pub fn append_frame_crc(frame_without_crc: &mut Vec<u8>) {
    assert!(frame_without_crc.len() >= 2);
    let c = crc16(0, &frame_without_crc[2..]);
    frame_without_crc.extend_from_slice(&c.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appended_crc_verifies() {
        let mut f = vec![0x0b, 0x77, 0x12, 0x34, 0x56];
        append_frame_crc(&mut f);
        assert!(frame_crc_ok(&f));
        f[3] ^= 1;
        assert!(!frame_crc_ok(&f));
    }
}
