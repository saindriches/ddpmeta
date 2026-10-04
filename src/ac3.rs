//! AC-3 syncframe parser, ETSI TS 102 366 V1.4.1 clause 4.3 (syncinfo, bsi, audblk, auxdata,
//! errorcheck) with the Annex D alternate BSI for bsid 6. Every element is recorded.

use crate::blocks::{
    allocate_block, conventional, read_exponents, AllocState, ExpSet, Groups, SlotAlloc, CPL, LFE,
};
use crate::error::{invalid, Result};
use crate::frame::{read_tail, Codec, Frame};
use crate::syntax::{Cursor, Role};
use crate::tables::{ac3_frame_words, NFCHANS};

pub fn parse(data: &[u8]) -> Result<Frame> {
    if data.len() < 6 {
        return invalid("short AC-3 frame");
    }
    let mut cur = Cursor::new(data, data.len() * 8);
    // syncinfo
    let sync = cur.get_role("syncword", Role::Sync, -1, 16)?;
    if sync != 0x0B77 {
        return invalid("no sync word");
    }
    cur.get_role("crc1", Role::Crc1, -1, 16)?;
    let fscod = cur.get_role("fscod", Role::FrameSize, -1, 2)? as usize;
    let frmsizecod = cur.get_role("frmsizecod", Role::FrameSize, -1, 6)? as usize;
    let Some(words) = ac3_frame_words(fscod, frmsizecod) else {
        return invalid(format!("fscod {fscod} frmsizecod {frmsizecod}"));
    };
    let bytes = 2 * words;
    if data.len() < bytes {
        return invalid("truncated AC-3 frame");
    }
    cur.limit = bytes * 8;
    // bsi (4.3.2, or D.1.2 when bsid is 6)
    let bsid = cur.get("bsid", 5)? as u8;
    if bsid > 8 {
        return invalid(format!("AC-3 parser given bsid {bsid}"));
    }
    cur.get("bsmod", 3)?;
    let acmod = cur.get("acmod", 3)? as u8;
    if acmod & 1 == 1 && acmod != 1 {
        cur.get("cmixlev", 2)?;
    }
    if acmod & 4 != 0 {
        cur.get("surmixlev", 2)?;
    }
    if acmod == 2 {
        cur.get("dsurmod", 2)?;
    }
    let lfeon = cur.flag("lfeon")?;
    let programs = if acmod == 0 { 2 } else { 1 };
    for p in 0..programs {
        let (dn, ce, c, lc, lcd, ap, ml, rt) = if p == 0 {
            (
                "dialnorm",
                "compre",
                "compr",
                "langcode",
                "langcod",
                "audprodie",
                "mixlevel",
                "roomtyp",
            )
        } else {
            (
                "dialnorm2",
                "compr2e",
                "compr2",
                "langcod2e",
                "langcod2",
                "audprodi2e",
                "mixlevel2",
                "roomtyp2",
            )
        };
        cur.get_role(dn, Role::Dialnorm(p), -1, 5)?;
        if cur.get_role(ce, Role::ComprExists(p), -1, 1)? == 1 {
            cur.get_role(c, Role::Compr(p), -1, 8)?;
        }
        if cur.flag(lc)? {
            cur.get(lcd, 8)?;
        }
        if cur.flag(ap)? {
            cur.get(ml, 5)?;
            cur.get(rt, 2)?;
        }
    }
    cur.get("copyrightb", 1)?;
    cur.get("origbs", 1)?;
    if bsid == 6 {
        if cur.flag("xbsi1e")? {
            cur.get("dmixmod", 2)?;
            cur.get("ltrtcmixlev", 3)?;
            cur.get("ltrtsurmixlev", 3)?;
            cur.get("lorocmixlev", 3)?;
            cur.get("lorosurmixlev", 3)?;
        }
        if cur.flag("xbsi2e")? {
            cur.get("dsurexmod", 2)?;
            cur.get("dheadphonmod", 2)?;
            cur.get("adconvtyp", 1)?;
            cur.get("xbsi2", 8)?;
            cur.get("encinfo", 1)?;
        }
    } else {
        if cur.flag("timecod1e")? {
            cur.get("timecod1", 14)?;
        }
        if cur.flag("timecod2e")? {
            cur.get("timecod2", 14)?;
        }
    }
    if cur.flag("addbsie")? {
        let l = cur.get("addbsil", 6)? as usize;
        cur.span("addbsi", Role::Addbsi, (l + 1) * 8)?;
    }

    let nfchans = NFCHANS[acmod as usize];
    let mut st = AllocState::default();
    let mut exps: [Option<ExpSet>; 7] = Default::default();
    let mut chbwcod = [None::<u32>; 5];
    // Coupling state carried across blocks (4.4.3.7).
    let mut cplinu = false;
    let mut chincpl = [false; 5];
    let mut phsflginu = false;
    let mut cplbegf = 0usize;
    let mut cplendf = 0usize;
    let mut ncplbnd = 0usize;
    let mut block_starts = Vec::with_capacity(6);
    let mut mantissa_starts = Vec::with_capacity(6);
    let mut cpl_blocks = 0;

    for blk in 0..6usize {
        cur.block = blk as i8;
        block_starts.push(cur.pos);
        for ch in 0..nfchans {
            cur.geti("blksw", ch, 1)?;
        }
        for ch in 0..nfchans {
            cur.geti("dithflag", ch, 1)?;
        }
        for p in 0..programs {
            let (e, v) = if p == 0 {
                ("dynrnge", "dynrng")
            } else {
                ("dynrng2e", "dynrng2")
            };
            if cur.get_role(e, Role::DynrngExists(p), -1, 1)? == 1 {
                cur.get_role(v, Role::Dynrng(p), -1, 8)?;
            }
        }
        // Coupling strategy.
        let cplstre = cur.flag("cplstre")?;
        if blk == 0 && !cplstre {
            return invalid("cplstre 0 in block 0");
        }
        if cplstre {
            cplinu = cur.flag("cplinu")?;
            if cplinu {
                for (ch, c) in chincpl.iter_mut().enumerate().take(nfchans) {
                    *c = cur.flagi("chincpl", ch)?;
                }
                phsflginu = if acmod == 2 {
                    cur.flag("phsflginu")?
                } else {
                    false
                };
                cplbegf = cur.get("cplbegf", 4)? as usize;
                cplendf = cur.get("cplendf", 4)? as usize;
                if cplbegf > cplendf + 2 {
                    return invalid("cplbegf beyond cplendf + 2");
                }
                let ncplsubnd = 3 + cplendf - cplbegf;
                ncplbnd = ncplsubnd;
                for bnd in 1..ncplsubnd {
                    if cur.flagi("cplbndstrc", bnd)? {
                        ncplbnd -= 1;
                    }
                }
                if chincpl.iter().take(nfchans).filter(|&&c| c).count() < 2 {
                    return invalid("coupling with fewer than two channels");
                }
            } else {
                chincpl = [false; 5];
                phsflginu = false;
            }
        }
        if cplinu {
            cpl_blocks += 1;
            let mut cplcoe = [false; 5];
            for ch in 0..nfchans {
                if chincpl[ch] {
                    cplcoe[ch] = cur.flagi("cplcoe", ch)?;
                    if cplcoe[ch] {
                        cur.geti("mstrcplco", ch, 2)?;
                        for _ in 0..ncplbnd {
                            cur.geti("cplcoexp", ch, 4)?;
                            cur.geti("cplcomant", ch, 4)?;
                        }
                    }
                }
            }
            if acmod == 2 && phsflginu && (cplcoe[0] || cplcoe[1]) {
                for bnd in 0..ncplbnd {
                    cur.geti("phsflg", bnd, 1)?;
                }
            }
        }
        // Rematrixing (4.4.3.19, Table 4.10).
        if acmod == 2 {
            let rematstr = cur.flag("rematstr")?;
            if blk == 0 && !rematstr {
                return invalid("rematstr 0 in block 0");
            }
            if rematstr {
                let n = if !cplinu || cplbegf > 2 {
                    4
                } else if cplbegf > 0 {
                    3
                } else {
                    2
                };
                for rbnd in 0..n {
                    cur.geti("rematflg", rbnd, 1)?;
                }
            }
        }
        // Exponent strategies.
        let cplexpstr = if cplinu {
            cur.get("cplexpstr", 2)? as u8
        } else {
            0
        };
        let mut chexpstr = [0u8; 5];
        for (ch, s) in chexpstr.iter_mut().enumerate().take(nfchans) {
            *s = cur.geti("chexpstr", ch, 2)? as u8;
        }
        let lfeexpstr = if lfeon {
            cur.get("lfeexpstr", 1)? as u8
        } else {
            0
        };
        for ch in 0..nfchans {
            if chexpstr[ch] != 0 && !chincpl[ch] {
                let c = cur.geti("chbwcod", ch, 6)?;
                if c > 60 {
                    return invalid(format!("chbwcod {c} above 60"));
                }
                chbwcod[ch] = Some(c);
            }
        }
        let cplstrt = 37 + 12 * cplbegf;
        let cplend = 37 + 12 * (cplendf + 3);
        let mut endmant = [0usize; 5];
        for ch in 0..nfchans {
            endmant[ch] = if chincpl[ch] && cplinu {
                cplstrt
            } else {
                match chbwcod[ch] {
                    Some(c) => 37 + 3 * (c as usize + 12),
                    None => return invalid("channel bandwidth unknown"),
                }
            };
        }
        // Exponents.
        if cplinu {
            if cplexpstr != 0 {
                exps[CPL] = Some(read_exponents(&mut cur, CPL, cplexpstr, cplstrt, cplend)?);
            } else if exps[CPL]
                .as_ref()
                .is_none_or(|e| e.start != cplstrt || e.end != cplend)
            {
                return invalid("coupling exponent reuse across a changed range");
            }
        }
        for ch in 0..nfchans {
            if chexpstr[ch] != 0 {
                exps[ch] = Some(read_exponents(&mut cur, ch, chexpstr[ch], 0, endmant[ch])?);
                cur.geti("gainrng", ch, 2)?;
            } else if exps[ch].as_ref().is_none_or(|e| e.end != endmant[ch]) {
                return invalid("channel exponent reuse across a changed range");
            }
        }
        if lfeon {
            if lfeexpstr != 0 {
                exps[LFE] = Some(read_exponents(&mut cur, LFE, 1, 0, 7)?);
            } else if exps[LFE].is_none() {
                return invalid("LFE exponent reuse in block 0");
            }
        }
        // Bit allocation parametric information.
        if cur.flag("baie")? {
            st.bai = Some([
                cur.get("sdcycod", 2)? as u8,
                cur.get("fdcycod", 2)? as u8,
                cur.get("sgaincod", 2)? as u8,
                cur.get("dbpbcod", 2)? as u8,
                cur.get("floorcod", 3)? as u8,
            ]);
        } else if blk == 0 {
            return invalid("baie 0 in block 0");
        }
        if cur.flag("snroffste")? {
            st.csnroffst = Some(cur.get("csnroffst", 6)? as u8);
            if cplinu {
                st.fsnroffst[CPL] = cur.get("cplfsnroffst", 4)? as u8;
                st.fgaincod[CPL] = cur.get("cplfgaincod", 3)? as u8;
            }
            for ch in 0..nfchans {
                st.fsnroffst[ch] = cur.geti("fsnroffst", ch, 4)? as u8;
                st.fgaincod[ch] = cur.geti("fgaincod", ch, 3)? as u8;
            }
            if lfeon {
                st.fsnroffst[LFE] = cur.get("lfefsnroffst", 4)? as u8;
                st.fgaincod[LFE] = cur.get("lfefgaincod", 3)? as u8;
            }
        } else if blk == 0 {
            return invalid("snroffste 0 in block 0");
        }
        if cplinu && cur.flag("cplleake")? {
            st.cplleak = Some((cur.get("cplfleak", 3)? as u8, cur.get("cplsleak", 3)? as u8));
        }
        // Delta bit allocation (4.4.3.47).
        if cur.flag("deltbaie")? {
            let mut codes = [0u32; 7];
            if cplinu {
                codes[CPL] = cur.get("cpldeltbae", 2)?;
                st.delta_code(CPL, codes[CPL], blk == 0)?;
            }
            for ch in 0..nfchans {
                codes[ch] = cur.geti("deltbae", ch, 2)?;
                st.delta_code(ch, codes[ch], blk == 0)?;
            }
            if cplinu && codes[CPL] == 1 {
                st.read_delta(&mut cur, CPL)?;
            }
            for ch in 0..nfchans {
                if codes[ch] == 1 {
                    st.read_delta(&mut cur, ch)?;
                }
            }
        } else if blk == 0 {
            for d in st.delta.iter_mut() {
                d.clear();
            }
        }
        // Skip field.
        if cur.get_role("skiple", Role::Skiple, -1, 1)? == 1 {
            let l = cur.get_role("skipl", Role::Skipl, -1, 9)? as usize;
            cur.span("skipfld", Role::Skipfld, 8 * l)?;
        }
        // Bit allocation for this block.
        let mut req = Vec::with_capacity(7);
        for ch in 0..nfchans {
            req.push(SlotAlloc {
                slot: ch,
                exps: exps[ch].as_ref().expect("checked above"),
                start: 0,
                end: endmant[ch],
                high_efficiency: false,
            });
        }
        if cplinu {
            req.push(SlotAlloc {
                slot: CPL,
                exps: exps[CPL].as_ref().expect("checked above"),
                start: cplstrt,
                end: cplend,
                high_efficiency: false,
            });
        }
        if lfeon {
            req.push(SlotAlloc {
                slot: LFE,
                exps: exps[LFE].as_ref().expect("checked above"),
                start: 0,
                end: 7,
                high_efficiency: false,
            });
        }
        let bap = allocate_block(&st, fscod, &req)?;
        // Mantissas (4.3.3, 6.3.5).
        let start = cur.pos;
        mantissa_starts.push(start);
        let mut g = Groups::default();
        let mut got_cpl = false;
        for ch in 0..nfchans {
            conventional(&mut cur, &bap[ch], 0, endmant[ch], &mut g)?;
            if cplinu && chincpl[ch] && !got_cpl {
                conventional(&mut cur, &bap[CPL], cplstrt, cplend, &mut g)?;
                got_cpl = true;
            }
        }
        if lfeon {
            conventional(&mut cur, &bap[LFE], 0, 7, &mut g)?;
        }
        cur.close_span("mantissas", Role::Mantissas, start);
    }
    let audio_end = cur.pos;
    read_tail(&mut cur, bytes * 8, Codec::Ac3)?;
    Ok(Frame {
        codec: Codec::Ac3,
        bytes,
        fields: cur.fields,
        bsid,
        acmod,
        lfeon,
        fscod: fscod as u8,
        strmtyp: 0,
        substreamid: 0,
        numblks: 6,
        block_starts,
        mantissa_starts,
        audio_end,
        aht_tags: 0,
        aht_slots: Vec::new(),
        spx_blocks: 0,
        cpl_blocks,
    })
}
