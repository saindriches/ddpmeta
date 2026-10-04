//! Tables. The bit allocation tables are in `bitalloc_tables`; the unit tests below check them
//! against the printed tables of ETSI TS 102 366 V1.4.1 (Tables 6.6 to 6.16 and E.2.1). The other
//! tables are transcribed from the clauses cited at each item.

pub use crate::bitalloc_tables::{
    BAND_END, BAP, DB_KNEE, FAST_DECAY, FAST_GAIN, FLOOR, HEARING, HEBAP, LOG_ADD, SLOW_DECAY,
    SLOW_GAIN,
};

/// Number of full bandwidth channels by acmod (4.4.2.3, Table 4.3).
pub const NFCHANS: [usize; 8] = [2, 1, 2, 3, 3, 4, 4, 5];

/// AC-3 words per syncframe by fscod and frmsizecod (4.4.4.1, Table 4.13).
pub fn ac3_frame_words(fscod: usize, frmsizecod: usize) -> Option<usize> {
    const W48: [u16; 19] = [
        64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024, 1152,
        1280,
    ];
    const W441: [u16; 38] = [
        69, 70, 87, 88, 104, 105, 121, 122, 139, 140, 174, 175, 208, 209, 243, 244, 278, 279, 348,
        349, 417, 418, 487, 488, 557, 558, 696, 697, 835, 836, 975, 976, 1114, 1115, 1253, 1254,
        1393, 1394,
    ];
    if frmsizecod >= 38 {
        return None;
    }
    match fscod {
        0 => Some(usize::from(W48[frmsizecod / 2])),
        1 => Some(usize::from(W441[frmsizecod])),
        2 => Some(usize::from(W48[frmsizecod / 2]) * 3 / 2),
        _ => None,
    }
}

/// Conventional mantissa bits by bap (Table 6.17). Grouped baps 1, 2 and 4 are handled by the
/// caller: 5 bits per three, 7 bits per three, 7 bits per two (6.3.5).
pub const BAP_BITS: [usize; 16] = [0, 0, 0, 3, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16];

/// AHT vector index bits by hebap 0 to 7 (Table E.2.2).
pub const VQ_BITS: [usize; 8] = [0, 2, 3, 4, 5, 7, 8, 9];

/// GAQ mantissa bits m by hebap 8 to 19 (Table E.2.2), indexed by hebap - 8.
pub const GAQ_BITS: [usize; 12] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16];

/// Exponent strategy codes: 0 reuse, 1 D15, 2 D25, 3 D45 (Table 6.4).
pub const REUSE: u8 = 0;

/// Frame exponent strategy combinations (Table E.1.9), blocks 0 to 5.
pub const FRAME_EXPSTR: [[u8; 6]; 32] = [
    [1, 0, 0, 0, 0, 0],
    [1, 0, 0, 0, 0, 3],
    [1, 0, 0, 0, 2, 0],
    [1, 0, 0, 0, 3, 3],
    [2, 0, 0, 2, 0, 0],
    [2, 0, 0, 2, 0, 3],
    [2, 0, 0, 3, 2, 0],
    [2, 0, 0, 3, 3, 3],
    [2, 0, 1, 0, 0, 0],
    [2, 0, 2, 0, 0, 3],
    [2, 0, 2, 0, 2, 0],
    [2, 0, 2, 0, 3, 3],
    [2, 0, 3, 2, 0, 0],
    [2, 0, 3, 2, 0, 3],
    [2, 0, 3, 3, 2, 0],
    [2, 0, 3, 3, 3, 3],
    [3, 1, 0, 0, 0, 0],
    [3, 1, 0, 0, 0, 3],
    [3, 2, 0, 0, 2, 0],
    [3, 2, 0, 0, 3, 3],
    [3, 2, 0, 2, 0, 0],
    [3, 2, 0, 2, 0, 3],
    [3, 2, 0, 3, 2, 0],
    [3, 2, 0, 3, 3, 3],
    [3, 3, 1, 0, 0, 0],
    [3, 3, 2, 0, 0, 3],
    [3, 3, 2, 0, 2, 0],
    [3, 3, 2, 0, 3, 3],
    [3, 3, 3, 2, 0, 0],
    [3, 3, 3, 2, 0, 3],
    [3, 3, 3, 3, 2, 0],
    [3, 3, 3, 3, 3, 3],
];

/// Default E-AC-3 coupling banding structure by coupling sub-band (Table E.1.12).
pub const DEF_CPL_BNDSTRC: [bool; 18] = [
    false, false, false, false, false, false, false, false, true, false, true, true, false, true,
    true, true, true, true,
];

/// Default spectral extension banding structure by SPX sub-band (Table E.1.10).
pub const DEF_SPX_BNDSTRC: [bool; 17] = [
    false, false, false, false, false, false, false, false, true, false, true, false, true, false,
    true, false, true,
];

/// First mantissa bin of 1/6 octave band `k` (Table 6.12 bndtab).
pub fn band_start(k: usize) -> usize {
    if k == 0 {
        0
    } else {
        BAND_END[k - 1] as usize
    }
}

/// Band holding mantissa bin `bin` (Table 6.13 masktab).
pub fn masktab(bin: usize) -> usize {
    BAND_END.partition_point(|&end| end as usize <= bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_allocation_tables_match_printed_specification() {
        // Tables 6.6 to 6.11.
        assert_eq!(SLOW_DECAY, [0x0f, 0x11, 0x13, 0x15]);
        assert_eq!(FAST_DECAY, [0x3f, 0x53, 0x67, 0x7b]);
        assert_eq!(SLOW_GAIN, [0x540, 0x4d8, 0x478, 0x410]);
        assert_eq!(DB_KNEE, [0, 0x700, 0x900, 0xb00]);
        assert_eq!(
            FLOOR,
            [
                0x2f0,
                0x2b0,
                0x270,
                0x230,
                0x1f0,
                0x170,
                0x0f0,
                0xf800u16 as i16
            ]
        );
        assert_eq!(
            FAST_GAIN,
            [0x80, 0x100, 0x180, 0x200, 0x280, 0x300, 0x380, 0x400]
        );
        // Table 6.12: band 28 starts at 28 and spans 3, band 49 starts at 229 and spans 24.
        assert_eq!((band_start(28), BAND_END[28]), (28, 31));
        assert_eq!((band_start(49), BAND_END[49]), (229, 253));
        // Table 6.13 samples.
        assert_eq!(masktab(29), 28);
        assert_eq!(masktab(36), 30);
        assert_eq!(masktab(252), 49);
        // Table 6.14 samples.
        assert_eq!(
            (LOG_ADD[0], LOG_ADD[13], LOG_ADD[209], LOG_ADD[210]),
            (0x40, 0x34, 1, 0)
        );
        // Table 6.15 samples, all three sample rates.
        assert_eq!(
            (HEARING[0], HEARING[50], HEARING[100]),
            (0x4d0, 0x4f0, 0x580)
        );
        assert_eq!(
            (HEARING[40], HEARING[90], HEARING[140]),
            (0x460, 0x420, 0x330)
        );
        // Table 6.16, complete.
        let baptab: [u8; 64] = [
            0, 1, 1, 1, 1, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 6, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8, 8, 9, 9,
            9, 9, 10, 10, 10, 10, 11, 11, 11, 11, 12, 12, 12, 12, 13, 13, 13, 13, 14, 14, 14, 14,
            14, 14, 14, 14, 15, 15, 15, 15, 15, 15, 15, 15, 15,
        ];
        assert_eq!(BAP, baptab);
        // Table E.2.1, complete.
        let mut hebaptab = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8, 9, 9, 9];
        for v in 10..=17u8 {
            hebaptab.extend([v; 4]);
        }
        hebaptab.extend([18; 8]);
        hebaptab.extend([19; 9]);
        assert_eq!(HEBAP.to_vec(), hebaptab);
    }

    #[test]
    fn ac3_frame_sizes() {
        assert_eq!(ac3_frame_words(0, 28), Some(768));
        assert_eq!(ac3_frame_words(1, 1), Some(70));
        assert_eq!(ac3_frame_words(2, 0), Some(96));
        assert_eq!(ac3_frame_words(0, 38), None);
    }
}
