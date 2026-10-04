// Only relevant to the `embed-key` feature: `src/emdf_protection.rs` embeds the key set this script
// writes to OUT_DIR. The key set comes from exactly one of
//   EMDF_EMBED_KEY_B64   the binary key file, base64 encoded (a CI secret is a string), or
//   EMDF_EMBED_KEY_FILE  the path of the binary key file.
// Nothing about the key is ever printed; errors name the variable, never its content. Without the
// feature this script writes nothing and the build carries no key.
use std::env;
use std::fs;
use std::path::Path;

const B64: &str = "EMDF_EMBED_KEY_B64";
const FILE: &str = "EMDF_EMBED_KEY_FILE";

fn main() {
    println!("cargo:rerun-if-env-changed={B64}");
    println!("cargo:rerun-if-env-changed={FILE}");
    if env::var_os("CARGO_FEATURE_EMBED_KEY").is_none() {
        return;
    }
    let b64 = env::var(B64).ok().filter(|v| !v.trim().is_empty());
    let file = env::var(FILE).ok().filter(|v| !v.is_empty());
    let keys = match (b64, file) {
        (Some(text), None) => {
            decode_base64(&text).unwrap_or_else(|e| panic!("{B64} is not valid base64: {e}"))
        }
        (None, Some(path)) => {
            println!("cargo:rerun-if-changed={path}");
            fs::read(&path).unwrap_or_else(|e| panic!("{FILE}: cannot read the key file: {e}"))
        }
        (Some(_), Some(_)) => {
            panic!("building --features embed-key: set {B64} or {FILE}, not both")
        }
        (None, None) => panic!(
            "building --features embed-key needs {B64} (the binary key file, base64) or {FILE} \
             (its path)"
        ),
    };
    if keys.is_empty() {
        panic!("building --features embed-key: the key set is empty");
    }
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("embedded_keys.bin");
    fs::write(&out, keys).expect("write the embedded key set to OUT_DIR");
}

/// Standard base64 (RFC 4648, `+` and `/`), padding optional, whitespace ignored.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut padding = false;
    for (i, c) in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .enumerate()
    {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                padding = true;
                continue;
            }
            _ => return Err(format!("unexpected character at position {i}")),
        };
        if padding {
            return Err(format!("data after padding at position {i}"));
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if bits >= 6 {
        return Err("truncated input".into());
    }
    Ok(out)
}
