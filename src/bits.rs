//! MSB-first bit access over byte buffers, as used by AC-3 and E-AC-3
//! (ETSI TS 102 366 V1.4.1, 4.3: "all bit stream elements arrive most significant bit first").

/// Read `n` (at most 32) bits starting at bit `pos`. The caller checks bounds.
pub fn get(data: &[u8], pos: usize, n: usize) -> u32 {
    debug_assert!(n <= 32);
    if n == 0 {
        return 0;
    }
    // At most 32 bits span at most five bytes, which fit a u64.
    let (first, last) = (pos / 8, (pos + n - 1) / 8);
    let v = data[first..=last]
        .iter()
        .fold(0u64, |v, &b| v << 8 | u64::from(b));
    let width = (last - first + 1) * 8;
    ((v >> (width - pos % 8 - n)) & ((1u64 << n) - 1)) as u32
}

/// Overwrite `n` (at most 32) bits starting at bit `pos` with the low bits of `value`.
pub fn set(data: &mut [u8], pos: usize, n: usize, value: u32) {
    debug_assert!(n <= 32);
    for i in 0..n {
        let p = pos + i;
        let bit = (value >> (n - 1 - i)) & 1;
        let mask = 1u8 << (7 - p % 8);
        if bit == 1 {
            data[p / 8] |= mask;
        } else {
            data[p / 8] &= !mask;
        }
    }
}

/// True when bits `[a, a+n)` of `x` equal bits `[b, b+n)` of `y`.
pub fn range_eq(x: &[u8], a: usize, y: &[u8], b: usize, n: usize) -> bool {
    let mut i = 0;
    while i < n {
        let k = (n - i).min(32);
        if get(x, a + i, k) != get(y, b + i, k) {
            return false;
        }
        i += k;
    }
    true
}

/// True when bits `[a, a+n)` of `x` are all zero.
pub fn range_zero(x: &[u8], a: usize, n: usize) -> bool {
    let mut i = 0;
    while i < n {
        let k = (n - i).min(32);
        if get(x, a + i, k) != 0 {
            return false;
        }
        i += k;
    }
    true
}

/// Growable MSB-first writer.
#[derive(Default, Clone, Debug)]
pub struct Writer {
    buf: Vec<u8>,
    len: usize,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn put(&mut self, n: usize, value: u32) {
        debug_assert!(n <= 32);
        let mut left = n;
        while left > 0 {
            if self.len.is_multiple_of(8) {
                self.buf.push(0);
            }
            // Fill the rest of the last byte, or as much of it as the value has left.
            let free = 8 - self.len % 8;
            let k = left.min(free);
            let chunk = (u64::from(value) >> (left - k)) & ((1 << k) - 1);
            let last = self.buf.len() - 1;
            self.buf[last] |= (chunk as u8) << (free - k);
            self.len += k;
            left -= k;
        }
    }

    pub fn zeros(&mut self, mut n: usize) {
        while n > 0 {
            let k = n.min(32);
            self.put(k, 0);
            n -= k;
        }
    }

    /// Append bits `[pos, pos+n)` of `src`.
    pub fn copy(&mut self, src: &[u8], pos: usize, mut n: usize) {
        let mut p = pos;
        while n > 0 {
            let k = n.min(32);
            self.put(k, get(src, p, k));
            p += k;
            n -= k;
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get` and `Writer::put` against a bit at a time reference, at every alignment and width.
    #[test]
    fn word_access_matches_bitwise() {
        let data: Vec<u8> = (0..16u32).map(|i| (i * 0x9d + 0x37) as u8).collect();
        for pos in 0..64 {
            for n in 0..=32 {
                let want = (0..n).fold(0u64, |v, i| {
                    v << 1 | u64::from(data[(pos + i) / 8] >> (7 - (pos + i) % 8) & 1)
                });
                assert_eq!(u64::from(get(&data, pos, n)), want, "get {pos} {n}");
                let mut w = Writer::new();
                w.put(pos % 13, 0x1fff);
                w.put(n, get(&data, pos, n) | if n < 32 { 1 << n } else { 0 });
                assert_eq!(w.len(), pos % 13 + n);
                let bytes = w.into_bytes();
                assert_eq!(get(&bytes, 0, pos % 13), (1 << (pos % 13)) - 1);
                assert_eq!(
                    get(&bytes, pos % 13, n),
                    get(&data, pos, n),
                    "put {pos} {n}"
                );
            }
        }
    }

    #[test]
    fn get_set_round_trip() {
        let mut d = vec![0u8; 4];
        set(&mut d, 3, 11, 0x5a5);
        assert_eq!(get(&d, 3, 11), 0x5a5);
        assert_eq!(get(&d, 0, 3), 0);
        set(&mut d, 3, 11, 0);
        assert!(range_zero(&d, 0, 32));
    }

    #[test]
    fn writer_copy_unaligned() {
        let src = [0b1011_0111u8, 0b0000_1111];
        let mut w = Writer::new();
        w.put(3, 0b101);
        w.copy(&src, 2, 9);
        assert_eq!(w.len(), 12);
        let b = w.into_bytes();
        assert_eq!(get(&b, 0, 3), 0b101);
        assert!(range_eq(&b, 3, &src, 2, 9));
    }
}
