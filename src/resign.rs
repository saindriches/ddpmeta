//! Stream level re-signing: recompute the EMDF protection of every protected container with
//! the configured keys and fix the checksums. Unlike an edit, this does not check that the
//! input protection verifies first: it replaces whatever values are stored, so it can repair a
//! container whose protection went stale. Frames are processed one at a time, so memory does
//! not grow with the stream.

use std::io::{Read, Write};
use std::path::Path;

use crate::edit::{validate, Lowered, Mask};
use crate::emdf;
use crate::emdf_protection::KeySet;
use crate::error::{Error, Result};
use crate::frame::fix_crcs;
use crate::protect::{self, Integrity};
use crate::stream::{parse_frame, FrameReader};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub frames: usize,
    pub frames_changed: usize,
    pub containers_resigned: usize,
}

/// Re-signs one syncframe at a time; the report accumulates over the frames.
pub struct Resigner<'a> {
    key: &'a KeySet,
    rep: Report,
}

impl<'a> Resigner<'a> {
    pub fn new(key: &'a KeySet) -> Self {
        Resigner {
            key,
            rep: Report::default(),
        }
    }

    /// The re-signed syncframe `n` found at byte `at`, or `None` when it carries no protected
    /// container.
    pub fn frame(&mut self, n: usize, at: u64, od: &[u8]) -> Result<Option<Vec<u8>>> {
        let ctx = |e: Error| match e {
            Error::Truncated(s) => Error::Truncated(format!("frame {n} at byte {at}: {s}")),
            Error::Unsupported(s) => Error::Unsupported(format!("frame {n} at byte {at}: {s}")),
            Error::Invalid(s) => Error::Invalid(format!("frame {n} at byte {at}: {s}")),
            Error::NotSupportedYet(s) => {
                Error::NotSupportedYet(format!("frame {n} at byte {at}: {s}"))
            }
            Error::Protected(s) => Error::Protected(format!("frame {n} at byte {at}: {s}")),
            Error::Io(s) => Error::Io(s),
        };
        self.rep.frames += 1;
        let of = parse_frame(od).map_err(ctx)?;
        let containers = emdf::containers(&of, od);
        if !containers
            .iter()
            .any(|c| c.protection.is_some() || c.error.is_some())
        {
            return Ok(None);
        }
        let mut nd = od.to_vec();
        let k = protect::resign(&of, &mut nd, self.key).map_err(|s| ctx(Error::Protected(s)))?;
        fix_crcs(of.codec, &mut nd);

        let mut mask: Mask = Vec::new();
        for c in &containers {
            if let Some(p) = &c.protection {
                for i in 0..p.primary_bits + p.secondary_bits {
                    mask.push((c.frame_pos(p.body_pos + i), 1));
                }
            }
        }
        let checked = validate(&of, od, &nd, &Lowered::Patches(Vec::new()), &mask).map_err(ctx)?;

        for c in emdf::containers(&checked, &nd) {
            if c.protection.is_some() {
                let v = protect::verify(&checked, &nd, &c, Some(self.key));
                if v != Integrity::Valid {
                    return Err(ctx(Error::Protected(format!(
                        "re-signed container does not verify ({v})"
                    ))));
                }
            }
        }
        if nd != od {
            self.rep.frames_changed += 1;
        }
        self.rep.containers_resigned += k;
        Ok(Some(nd))
    }

    pub fn finish(self) -> Report {
        self.rep
    }
}

/// Re-sign a whole stream held in memory.
pub fn resign(data: &[u8], key: &KeySet) -> Result<(Vec<u8>, Report)> {
    let mut out = Vec::with_capacity(data.len());
    let rep = resign_stream(data, &mut out, key)?;
    Ok((out, rep))
}

/// Re-sign from a reader to a writer, one syncframe at a time. On an error the writer holds the
/// frames before the failing one.
pub fn resign_stream(r: impl Read, w: &mut impl Write, key: &KeySet) -> Result<Report> {
    let mut frames = FrameReader::new(r);
    let mut s = Resigner::new(key);
    while let Some(f) = frames.next_frame()? {
        let edited = s.frame(f.index, f.at, f.data)?;
        w.write_all(edited.as_deref().unwrap_or(f.data))
            .map_err(io)?;
    }
    w.flush().map_err(io)?;
    Ok(s.finish())
}

/// Re-sign a file in place. The file is written only once every syncframe has succeeded.
pub fn resign_in_place(path: &Path, key: &KeySet) -> Result<Report> {
    let mut s = Resigner::new(key);
    crate::strip::edit_in_place(path, |n, at, d| s.frame(n, at, d))?;
    Ok(s.finish())
}

fn io(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}
