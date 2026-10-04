//! Gain word and dialnorm values in dB, ETSI TS 102 366 V1.4.1 clauses 4.4.2.8, 6.7.2.2 and 6.7.3.2.

/// dialnorm in dB relative to full scale; the reserved code 0 is read as -31 dB (4.4.2.8).
pub fn dialnorm_db(code: u32) -> i32 {
    if code == 0 {
        -31
    } else {
        -(code as i32)
    }
}

/// dynrng gain in dB (6.7.2.2): X is the signed top 3 bits, Y the low 5 bits,
/// gain = 2^(X+1) * 0.1YYYYY (base 2). Code 0 is exactly unity gain.
pub fn dynrng_db(code: u32) -> f64 {
    let x = i32::from((code as u8 as i8) >> 5);
    let y = f64::from(code & 0x1f);
    20.0 * (2f64.powi(x + 1) * (0.5 + y / 64.0)).log10()
}

/// EMDF gated loudness code for a LUFS value (H.3.1.3.8, H.3.1.3.10): the scale is -58 LUFS
/// plus 0.5 LU per step, so code = (lufs + 58) / 0.5, rounded and clamped to the 7 bit range.
pub fn loudness_code(lufs: f64) -> u8 {
    ((lufs + 58.0) / 0.5).round().clamp(0.0, 127.0) as u8
}

/// compr gain in dB (6.7.3.2): X is the signed top 4 bits, Y the low 4 bits,
/// gain = 2^(X+1) * 0.1YYYY (base 2). Code 0 is exactly unity gain.
pub fn compr_db(code: u32) -> f64 {
    let x = i32::from((code as u8 as i8) >> 4);
    let y = f64::from(code & 0x0f);
    20.0 * (2f64.powi(x + 1) * (0.5 + y / 32.0)).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn ranges_match_clause_6_7() {
        // 6.7.2.2: "+23,94 dB" to "-24,06 dB", code 0 is unity.
        assert_eq!(dynrng_db(0), 0.0);
        assert!(close(dynrng_db(0x7f), 23.94));
        assert!(close(dynrng_db(0x80), -24.08));
        // Table 6.29: X = -1 (111) with Y = 0 is -6.02 dB (0 dB shift, Y = 1/2).
        assert!(close(dynrng_db(0xe0), -6.02));
        // 6.7.3.2: +47,88 dB to -48,14 dB.
        assert_eq!(compr_db(0), 0.0);
        assert!(close(compr_db(0x7f), 47.88));
        assert!(close(compr_db(0x80), -48.16));
        assert_eq!(dialnorm_db(0), -31);
        assert_eq!(dialnorm_db(24), -24);
    }

    #[test]
    fn loudness_code_matches_the_h_3_1_scale() {
        // -58 LUFS + 0.5 LU per step: -31 LUFS is code 54, -24 LUFS is code 68.
        assert_eq!(loudness_code(-31.0), 54);
        assert_eq!(loudness_code(-24.0), 68);
        assert_eq!(loudness_code(-58.0), 0);
        // out of range clamps into the 7 bit field.
        assert_eq!(loudness_code(10.0), 127);
        assert_eq!(loudness_code(-100.0), 0);
    }
}
