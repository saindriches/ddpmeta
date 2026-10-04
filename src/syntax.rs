//! A parsed syncframe is a contiguous list of syntax elements. Every bit of the frame belongs
//! to exactly one element, from the sync word to crc2, so a frame can be rebuilt element by
//! element and two parses can be compared element by element.

use crate::bits;
use crate::error::{Error, Result};

/// What an element means to the metadata editor. Everything not listed is `Plain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Plain,
    Sync,
    /// AC-3 crc1 (4.3.1).
    Crc1,
    /// crc2 (4.3.5, E.1.2.6).
    Crc2,
    /// AC-3 crcrsv or E-AC-3 encinfo, the bit before crc2.
    CrcRsv,
    /// E-AC-3 strmtyp, substreamid and frmsiz; AC-3 fscod and frmsizecod.
    FrameSize,
    /// dialnorm (programme 0) or dialnorm2 (programme 1).
    Dialnorm(u8),
    /// compre or compr2e.
    ComprExists(u8),
    /// compr or compr2.
    Compr(u8),
    /// dynrnge or dynrng2e.
    DynrngExists(u8),
    /// dynrng or dynrng2.
    Dynrng(u8),
    Skiple,
    Skipl,
    /// skipfld bytes, one span.
    Skipfld,
    /// addbsi bytes, one span.
    Addbsi,
    /// Every mantissa of one audio block (including AHT gain words and pre-mantissas), one span.
    Mantissas,
    /// auxbits, one span from the end of the last audio block to auxdatal (or auxdatae).
    AuxBits,
    AuxDataL,
    AuxDataE,
    BlkStrtInfoE,
    BlkStrtInfo,
}

impl Role {
    /// Elements that hold a gain word or its presence flag.
    pub fn is_drc(self) -> bool {
        matches!(
            self,
            Role::ComprExists(_) | Role::Compr(_) | Role::DynrngExists(_) | Role::Dynrng(_)
        )
    }

    /// Elements whose value is a whole-frame checksum and is recomputed after any edit.
    pub fn is_checksum(self) -> bool {
        matches!(self, Role::Crc1 | Role::Crc2)
    }

    /// Spans whose content is compared bit by bit rather than by value.
    pub fn is_span(self) -> bool {
        matches!(
            self,
            Role::Skipfld | Role::Addbsi | Role::Mantissas | Role::AuxBits
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub role: Role,
    /// Audio block, or -1 outside blocks.
    pub block: i8,
    /// Channel or array index where the syntax has one, else -1.
    pub index: i16,
    /// Bit position from the start of the syncframe (the first bit of the sync word).
    pub pos: usize,
    pub width: usize,
    /// Value for elements of at most 32 bits; 0 for spans.
    pub value: u32,
}

impl Field {
    pub fn end(&self) -> usize {
        self.pos + self.width
    }
}

/// Sequential reader that records every element it reads.
pub struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub limit: usize,
    pub fields: Vec<Field>,
    pub block: i8,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8], limit_bits: usize) -> Self {
        Self {
            data,
            pos: 0,
            limit: limit_bits,
            fields: Vec::with_capacity(1024),
            block: -1,
        }
    }

    fn check(&self, width: usize, name: &str) -> Result<()> {
        if self.pos + width > self.limit {
            return Err(Error::Truncated(format!(
                "{name} ({width} bits) at bit {} beyond {}",
                self.pos, self.limit
            )));
        }
        Ok(())
    }

    pub fn get_role(
        &mut self,
        name: &'static str,
        role: Role,
        index: i16,
        width: usize,
    ) -> Result<u32> {
        self.check(width, name)?;
        let value = bits::get(self.data, self.pos, width);
        self.fields.push(Field {
            name,
            role,
            block: self.block,
            index,
            pos: self.pos,
            width,
            value,
        });
        self.pos += width;
        Ok(value)
    }

    pub fn get(&mut self, name: &'static str, width: usize) -> Result<u32> {
        self.get_role(name, Role::Plain, -1, width)
    }

    pub fn geti(&mut self, name: &'static str, index: usize, width: usize) -> Result<u32> {
        self.get_role(name, Role::Plain, index as i16, width)
    }

    pub fn flag(&mut self, name: &'static str) -> Result<bool> {
        Ok(self.get(name, 1)? != 0)
    }

    pub fn flagi(&mut self, name: &'static str, index: usize) -> Result<bool> {
        Ok(self.geti(name, index, 1)? != 0)
    }

    /// Record `width` bits as one span element.
    pub fn span(&mut self, name: &'static str, role: Role, width: usize) -> Result<()> {
        self.check(width, name)?;
        self.fields.push(Field {
            name,
            role,
            block: self.block,
            index: -1,
            pos: self.pos,
            width,
            value: 0,
        });
        self.pos += width;
        Ok(())
    }

    /// Record a span that was walked with `peek`/`advance` from `start` to the cursor.
    pub fn close_span(&mut self, name: &'static str, role: Role, start: usize) {
        self.fields.push(Field {
            name,
            role,
            block: self.block,
            index: -1,
            pos: start,
            width: self.pos - start,
            value: 0,
        });
    }

    /// Read bits without recording an element (inside a span).
    pub fn raw(&mut self, width: usize, what: &str) -> Result<u32> {
        self.check(width, what)?;
        let v = bits::get(self.data, self.pos, width);
        self.pos += width;
        Ok(v)
    }
}

/// Verify that elements tile `[0, total)` without gaps or overlaps.
pub fn check_contiguous(fields: &[Field], total: usize) -> Result<()> {
    let mut at = 0;
    for f in fields {
        if f.pos != at {
            return Err(Error::Invalid(format!(
                "element {} at bit {} does not follow bit {at}",
                f.name, f.pos
            )));
        }
        at = f.end();
    }
    if at != total {
        return Err(Error::Invalid(format!(
            "elements end at bit {at}, frame has {total}"
        )));
    }
    Ok(())
}
