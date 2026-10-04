//! EMDF container protection shared by standard DD+ and DD+ JOC.
//!
//! Annex H of ETSI TS 102 366 leaves the protection values implementation dependent. DD+ and
//! DD+ JOC streams fill them the same way: primary protection is the first four bytes of
//! HMAC-SHA256 over the authenticated core bytes followed by the whole aligned container with both
//! protection values zero; secondary protection is the first byte of HMAC-SHA256 over that
//! container's full 32 byte primary digest.
//!
//! The key is a configuration input, the same for DD+ and DD+ JOC: this crate neither contains
//! nor derives it.

/// The EMDF protection key, supplied by the caller's configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct ProtectionKey(Vec<u8>);

impl ProtectionKey {
    pub fn new(bytes: Vec<u8>) -> Result<Self, &'static str> {
        if bytes.is_empty() {
            return Err("empty EMDF protection key");
        }
        Ok(Self(bytes))
    }
    pub fn from_hex(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("EMDF protection key is not hex");
        }
        let bytes = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect();
        Self::new(bytes)
    }
    /// Key length in bytes (the only property of the key that may be printed).
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Always false: `new` refuses an empty key.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for ProtectionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProtectionKey({} bytes)", self.0.len())
    }
}

/// EMDF protection keys indexed by their container `key_id`.
///
/// Two key ids are in use (0 and 1); the bitstream `key_id` is an open ended `variable_bits`
/// field, so this is a map rather than a fixed pair. Keys need not be any particular length: HMAC
/// accepts any, so each entry carries its own length. The on disk form is a small machine binary
/// (not text, to keep a stray `atoi`/`strtol` from reading it): an eight byte magic
/// `89 45 4D 4B 0D 0A 1A 0A`, a one byte version, a one byte entry count, then that many entries of
/// a little endian `u32` key id, a little endian `u16` byte length and the key bytes.
#[derive(Clone, Default)]
pub struct KeySet(std::collections::BTreeMap<u64, ProtectionKey>);

impl KeySet {
    /// File signature: high bit set so a decimal/hex text reader stops at once, plus the PNG style
    /// `\r\n\x1a\n` tail that catches text mode mangling.
    pub const MAGIC: [u8; 8] = [0x89, b'E', b'M', b'K', 0x0D, 0x0A, 0x1A, 0x0A];
    /// On disk format version.
    pub const FORMAT_VERSION: u8 = 1;

    pub fn new() -> Self {
        Self::default()
    }
    /// Add or replace the key for `key_id`.
    pub fn insert(&mut self, key_id: u64, key: ProtectionKey) {
        self.0.insert(key_id, key);
    }
    /// The key for `key_id`, if configured.
    pub fn get(&self, key_id: u64) -> Option<&ProtectionKey> {
        self.0.get(&key_id)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// The configured key ids, ascending.
    pub fn key_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.0.keys().copied()
    }

    /// Parse the machine binary key file.
    pub fn from_binary(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 10 || bytes[..8] != Self::MAGIC {
            return Err("not an EMDF key file (bad magic)");
        }
        if bytes[8] != Self::FORMAT_VERSION {
            return Err("unsupported EMDF key file version");
        }
        let count = bytes[9] as usize;
        let mut pos = 10;
        let mut set = Self::new();
        for _ in 0..count {
            if pos + 6 > bytes.len() {
                return Err("truncated EMDF key file entry header");
            }
            let key_id = u32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap()) as u64;
            let len = u16::from_le_bytes(bytes[pos + 4..pos + 6].try_into().unwrap()) as usize;
            pos += 6;
            if pos + len > bytes.len() {
                return Err("truncated EMDF key file key");
            }
            let key = ProtectionKey::new(bytes[pos..pos + len].to_vec())?;
            if set.0.insert(key_id, key).is_some() {
                return Err("duplicate key_id in EMDF key file");
            }
            pos += len;
        }
        if set.is_empty() {
            return Err("EMDF key file has no keys");
        }
        Ok(set)
    }

    /// Serialize to the machine binary key file. Key ids must fit `u32` and keys must be at most
    /// 65535 bytes (every real EMDF key is far smaller); more than 255 keys cannot be represented.
    pub fn to_binary(&self) -> Result<Vec<u8>, &'static str> {
        if self.0.len() > u8::MAX as usize {
            return Err("too many keys for the EMDF key file");
        }
        let mut out = Vec::new();
        out.extend_from_slice(&Self::MAGIC);
        out.push(Self::FORMAT_VERSION);
        out.push(self.0.len() as u8);
        for (&id, key) in &self.0 {
            let id = u32::try_from(id).map_err(|_| "key_id does not fit u32")?;
            let len = u16::try_from(key.0.len()).map_err(|_| "key too long for the key file")?;
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&key.0);
        }
        Ok(out)
    }
}

impl std::fmt::Debug for KeySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeySet({} keys)", self.0.len())
    }
}

/// Primary (four bytes) and secondary (one byte) protection values for a container whose
/// protection bits are zero in `blank`.
pub fn protect(key: &ProtectionKey, core: &[u8], blank: &[u8]) -> [u8; 5] {
    let mut authenticated = core.to_vec();
    authenticated.extend_from_slice(blank);
    let primary = hmac(&key.0, &authenticated);
    let secondary = hmac(&key.0, &primary);
    [primary[0], primary[1], primary[2], primary[3], secondary[0]]
}

/// The authenticated core of a frame: its bits in file order without the excluded inclusive bit
/// ranges, packed into 16 bit words. The kept bit count is rounded to the nearest word, halves
/// up: a final partial word of eight or more bits is zero padded, a shorter one dropped.
pub fn authenticated_core(
    frame: &[u8],
    excluded: &[(usize, usize)],
) -> Result<Vec<u8>, &'static str> {
    let total = frame.len() * 8;
    if excluded.iter().any(|&(a, b)| a > b || b >= total) {
        return Err("excluded bit range outside the frame");
    }
    let mut ranges = excluded.to_vec();
    ranges.sort_unstable();
    let mut out = Vec::with_capacity(frame.len());
    // Pending bits not yet a whole byte: `acc` holds `held` of them, low aligned.
    let (mut acc, mut held, mut kept) = (0u64, 0, 0);
    let mut copy = |from: usize, to: usize| {
        let mut p = from;
        while p < to {
            let k = (to - p).min(32);
            acc = acc << k | bits_at(frame, p, k);
            held += k;
            while held >= 8 {
                held -= 8;
                out.push((acc >> held) as u8);
            }
            acc &= (1 << held) - 1;
            kept += k;
            p += k;
        }
    };
    // Copy the runs between the excluded ranges, which may overlap.
    let mut next = 0;
    for &(a, b) in &ranges {
        if a > next {
            copy(next, a);
        }
        next = next.max(b + 1);
    }
    copy(next, total);
    if held != 0 {
        out.push((acc << (8 - held)) as u8);
    }
    out.resize((kept + 8) / 16 * 2, 0);
    Ok(out)
}

/// `k` (at most 32) bits of `frame` from bit `pos`, most significant first.
fn bits_at(frame: &[u8], pos: usize, k: usize) -> u64 {
    let (first, last) = (pos / 8, (pos + k - 1) / 8);
    let v = frame[first..=last]
        .iter()
        .fold(0u64, |v, &b| v << 8 | u64::from(b));
    (v >> ((last - first + 1) * 8 - pos % 8 - k)) & ((1 << k) - 1)
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    inner.extend(data);
    let mut outer: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    outer.extend(sha256(&inner));
    sha256(&outer)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    // FIPS 180-4 SHA-256 round constants.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend((data.len() as u64 * 8).to_be_bytes());
    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, b) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(b.try_into().unwrap());
        }
        for i in 16..64 {
            let x = w[i - 15];
            let y = w[i - 2];
            let s0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let s1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0; 32];
    for (i, w) in state.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha256_fips_180_example() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hmac_rfc_4231_case_2() {
        assert_eq!(
            hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn core_rounds_to_the_nearest_word() {
        let frame = [0xffu8; 4];
        // 23 kept bits: (23 + 8) / 16 = 1 word, the partial word dropped.
        assert_eq!(
            authenticated_core(&frame, &[(0, 8)]).unwrap(),
            vec![0xff, 0xff]
        );
        // 24 kept bits: 2 words, the second zero padded.
        assert_eq!(
            authenticated_core(&frame, &[(0, 7)]).unwrap(),
            vec![0xff, 0xff, 0xff, 0x00]
        );
        assert!(authenticated_core(&frame, &[(0, 32)]).is_err());
        // Unsorted and overlapping ranges, against a bit at a time reference.
        let frame: Vec<u8> = (0..40u32).map(|i| (i * 0x6b + 0x11) as u8).collect();
        let excluded = [(200, 211), (3, 3), (17, 40), (30, 60), (300, 319), (61, 61)];
        let kept: Vec<u8> = (0..frame.len() * 8)
            .filter(|&i| !excluded.iter().any(|&(a, b)| a <= i && i <= b))
            .map(|i| frame[i / 8] >> (7 - i % 8) & 1)
            .collect();
        let mut want: Vec<u8> = kept
            .chunks(8)
            .map(|c| c.iter().enumerate().fold(0, |v, (j, &b)| v | b << (7 - j)))
            .collect();
        want.resize((kept.len() + 8) / 16 * 2, 0);
        assert_eq!(authenticated_core(&frame, &excluded).unwrap(), want);
    }

    #[test]
    fn key_reports_only_its_length() {
        let k = ProtectionKey::from_hex("001122").unwrap();
        assert_eq!(format!("{k:?}"), "ProtectionKey(3 bytes)");
        assert_eq!(k.len(), 3);
        assert!(ProtectionKey::from_hex("abc").is_err());
        assert!(ProtectionKey::from_hex("").is_err());
    }

    #[test]
    fn keyset_binary_round_trips_with_mixed_lengths() {
        let mut set = KeySet::new();
        set.insert(0, ProtectionKey::new(vec![0xaa; 32]).unwrap());
        set.insert(1, ProtectionKey::new(vec![0xbb; 5]).unwrap()); // HMAC takes any length
        let bytes = set.to_binary().unwrap();
        assert_eq!(bytes[..8], KeySet::MAGIC);
        let back = KeySet::from_binary(&bytes).unwrap();
        assert_eq!(back.key_ids().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(back.get(0).unwrap().len(), 32);
        assert_eq!(back.get(1).unwrap().len(), 5);
        assert!(back.get(2).is_none());
    }

    #[test]
    fn keyset_rejects_non_binary_and_truncation() {
        // Hex text (no magic) is not a binary key file.
        assert!(KeySet::from_binary(b"00112233445566778899aabbccddeeff").is_err());
        let mut set = KeySet::new();
        set.insert(1, ProtectionKey::new(vec![0xcc; 32]).unwrap());
        let mut bytes = set.to_binary().unwrap();
        bytes.pop(); // drop a key byte
        assert!(KeySet::from_binary(&bytes).is_err());
    }
}
