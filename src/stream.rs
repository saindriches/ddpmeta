//! Splitting an elementary stream into syncframes and parsing each one.

use std::io::{ErrorKind, Read};

use crate::error::{invalid, Error, Result};
use crate::frame::{probe, Codec, Frame};
use crate::{ac3, eac3};

/// Parse one syncframe that starts at `data[0]`.
pub fn parse_frame(data: &[u8]) -> Result<Frame> {
    let (codec, len) = probe(data)?;
    if data.len() < len {
        return invalid(format!("frame of {len} bytes truncated to {}", data.len()));
    }
    match codec {
        Codec::Ac3 => ac3::parse(&data[..len]),
        Codec::Eac3 => eac3::parse(&data[..len]),
    }
}

/// Byte ranges of consecutive syncframes. The stream must start with a sync word and frames
/// must follow each other without gaps; anything else is refused.
pub fn split(data: &[u8]) -> Result<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let (_, len) = probe(&data[at..])
            .map_err(|e| crate::error::Error::Invalid(format!("at byte {at}: {e}")))?;
        if len < 6 || at + len > data.len() {
            return invalid(format!(
                "frame at byte {at} of {len} bytes overruns the stream"
            ));
        }
        out.push((at, len));
        at += len;
    }
    Ok(out)
}

/// Reads consecutive syncframes from a byte stream one at a time, so memory stays one frame
/// whatever the stream length. Same rules as [`split`]: the stream starts with a sync word and
/// frames follow each other without gaps.
pub struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
    at: u64,
    index: usize,
}

/// One syncframe read by [`FrameReader`]: its index, its byte offset in the stream and its bytes.
pub struct FrameRef<'a> {
    pub index: usize,
    pub at: u64,
    pub data: &'a [u8],
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        FrameReader {
            inner,
            buf: Vec::new(),
            at: 0,
            index: 0,
        }
    }

    /// The next syncframe, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<FrameRef<'_>>> {
        let at = self.at;
        // Six bytes hold everything `probe` reads: the sync word, the size fields and bsid.
        self.buf.resize(6, 0);
        match fill(&mut self.inner, &mut self.buf)? {
            0 => return Ok(None),
            6 => {}
            n => return invalid(format!("{n} bytes after the last frame at byte {at}")),
        }
        let (_, len) =
            probe(&self.buf).map_err(|e| Error::Invalid(format!("at byte {at}: {e}")))?;
        if len < 6 {
            return invalid(format!("frame at byte {at} of {len} bytes"));
        }
        self.buf.resize(len, 0);
        if fill(&mut self.inner, &mut self.buf[6..])? != len - 6 {
            return invalid(format!(
                "frame at byte {at} of {len} bytes overruns the stream"
            ));
        }
        self.at += len as u64;
        self.index += 1;
        Ok(Some(FrameRef {
            index: self.index - 1,
            at,
            data: &self.buf,
        }))
    }
}

/// Read until `buf` is full or the stream ends; the number of bytes read.
fn fill(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e.to_string())),
        }
    }
    Ok(n)
}
