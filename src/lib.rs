//! ddpmeta: show and strip DRC and dialogue normalization metadata in DD (AC-3) and DD+
//! (E-AC-3, including DD+ JOC) elementary streams. Syntax authority: ETSI TS 102 366 V1.4.1.
//! See README.md for usage.

// Index loops mirror the channel, block and bin loops of the specification's pseudocode.
#![allow(clippy::needless_range_loop)]

pub mod ac3;
pub mod alloc;
#[rustfmt::skip]
pub mod bitalloc_tables;
pub mod bits;
pub mod blocks;
pub mod crc;
pub mod eac3;
pub mod edit;
pub mod emdf;
pub mod emdf_protection;
mod emdf_protection_core;
pub mod error;
pub mod frame;
pub mod gain;
pub mod protect;
pub mod show;
pub mod stream;
pub mod strip;
pub mod syntax;
pub mod tables;
