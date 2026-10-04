//! E-AC-3 syncframe parser, ETSI TS 102 366 V1.4.1 Annex E: E.1.2.1 syncinfo, E.1.2.2 bsi,
//! E.1.2.3 audfrm, E.1.2.4 audblk, E.1.2.5 auxdata and E.1.2.6 errorcheck, with the helper
//! derivations of E.2.3 and E.2.4.2. Enhanced coupling is refused: no stream at hand uses it, so
//! its parsing could not be validated.

use crate::blocks::{
    aht, allocate_block, conventional, read_exponents, refuse, AllocState, ExpSet, Groups,
    SlotAlloc, CPL, LFE, SLOTS,
};
use crate::error::{invalid, Result};
use crate::frame::{read_tail, Codec, Frame};
use crate::syntax::{Cursor, Role};
use crate::tables::{DEF_CPL_BNDSTRC, DEF_SPX_BNDSTRC, FRAME_EXPSTR, NFCHANS};

pub fn parse(data: &[u8]) -> Result<Frame> {
    if data.len() < 6 {
        return invalid("short E-AC-3 frame");
    }
    let mut cur = Cursor::new(data, data.len() * 8);
    if cur.get_role("syncword", Role::Sync, -1, 16)? != 0x0B77 {
        return invalid("no sync word");
    }
    // bsi
    let strmtyp = cur.get_role("strmtyp", Role::FrameSize, -1, 2)? as u8;
    let substreamid = cur.get_role("substreamid", Role::FrameSize, -1, 3)? as u8;
    let frmsiz = cur.get_role("frmsiz", Role::FrameSize, -1, 11)? as usize;
    let bytes = 2 * (frmsiz + 1);
    if strmtyp == 3 {
        return invalid("reserved strmtyp 3");
    }
    if data.len() < bytes {
        return invalid("truncated E-AC-3 frame");
    }
    cur.limit = bytes * 8;
    let fscod = cur.get("fscod", 2)? as u8;
    if fscod == 3 {
        return refuse("reduced sample rate (fscod 3)");
    }
    let numblkscod = cur.get("numblkscod", 2)? as usize;
    let numblks = [1, 2, 3, 6][numblkscod];
    let acmod = cur.get("acmod", 3)? as u8;
    let lfeon = cur.flag("lfeon")?;
    let bsid = cur.get("bsid", 5)? as u8;
    if !(11..=16).contains(&bsid) {
        return invalid(format!("E-AC-3 parser given bsid {bsid}"));
    }
    cur.get_role("dialnorm", Role::Dialnorm(0), -1, 5)?;
    if cur.get_role("compre", Role::ComprExists(0), -1, 1)? == 1 {
        cur.get_role("compr", Role::Compr(0), -1, 8)?;
    }
    if acmod == 0 {
        cur.get_role("dialnorm2", Role::Dialnorm(1), -1, 5)?;
        if cur.get_role("compr2e", Role::ComprExists(1), -1, 1)? == 1 {
            cur.get_role("compr2", Role::Compr(1), -1, 8)?;
        }
    }
    if strmtyp == 1 && cur.flag("chanmape")? {
        cur.get("chanmap", 16)?;
    }
    if cur.flag("mixmdate")? {
        if acmod > 2 {
            cur.get("dmixmod", 2)?;
        }
        if acmod & 1 == 1 && acmod > 2 {
            cur.get("ltrtcmixlev", 3)?;
            cur.get("lorocmixlev", 3)?;
        }
        if acmod & 4 != 0 {
            cur.get("ltrtsurmixlev", 3)?;
            cur.get("lorosurmixlev", 3)?;
        }
        if lfeon && cur.flag("lfemixlevcode")? {
            cur.get("lfemixlevcod", 5)?;
        }
        if strmtyp == 0 {
            if cur.flag("pgmscle")? {
                cur.get("pgmscl", 6)?;
            }
            if acmod == 0 && cur.flag("pgmscl2e")? {
                cur.get("pgmscl2", 6)?;
            }
            if cur.flag("extpgmscle")? {
                cur.get("extpgmscl", 6)?;
            }
            let mixdef = cur.get("mixdef", 2)?;
            match mixdef {
                1 => {
                    cur.get("premixcmpsel", 1)?;
                    cur.get("drcsrc", 1)?;
                    cur.get("premixcmpscl", 3)?;
                }
                2 => {
                    cur.get("mixdata", 12)?;
                }
                3 => {
                    let mixdeflen = cur.get("mixdeflen", 5)? as usize;
                    let start = cur.pos;
                    if cur.flag("mixdata2e")? {
                        cur.get("premixcmpsel", 1)?;
                        cur.get("drcsrc", 1)?;
                        cur.get("premixcmpscl", 3)?;
                        for name in [
                            "extpgmlscl",
                            "extpgmcscl",
                            "extpgmrscl",
                            "extpgmlsscl",
                            "extpgmrsscl",
                            "extpgmlfescl",
                            "dmixscl",
                        ] {
                            if cur.flag("scale_exists")? {
                                cur.get(name, 4)?;
                            }
                        }
                        if cur.flag("addche")? {
                            for name in ["extpgmaux1scl", "extpgmaux2scl"] {
                                if cur.flag("scale_exists")? {
                                    cur.get(name, 4)?;
                                }
                            }
                        }
                    }
                    if cur.flag("mixdata3e")? {
                        cur.get("spchdat", 5)?;
                        if cur.flag("addspchdate")? {
                            cur.get("spchdat1", 5)?;
                            cur.get("spchan1att", 2)?;
                            if cur.flag("addspchdat1e")? {
                                cur.get("spchdat2", 5)?;
                                cur.get("spchan2att", 3)?;
                            }
                        }
                    }
                    let total = 8 * (mixdeflen + 2);
                    let used = cur.pos - start;
                    if used > total {
                        return invalid("mixdata fields exceed mixdeflen");
                    }
                    // Remaining mixdata bits and mixdatafill, kept as one plain span.
                    cur.span("mixdata", Role::Plain, total - used)?;
                }
                _ => {}
            }
            if acmod < 2 {
                if cur.flag("paninfoe")? {
                    cur.get("panmean", 8)?;
                    cur.get("paninfo", 6)?;
                }
                if acmod == 0 && cur.flag("paninfo2e")? {
                    cur.get("panmean2", 8)?;
                    cur.get("paninfo2", 6)?;
                }
            }
            if cur.flag("frmmixcfginfoe")? {
                if numblkscod == 0 {
                    cur.get("blkmixcfginfo", 5)?;
                } else {
                    for blk in 0..numblks {
                        if cur.flagi("blkmixcfginfoe", blk)? {
                            cur.geti("blkmixcfginfo", blk, 5)?;
                        }
                    }
                }
            }
        }
    }
    if cur.flag("infomdate")? {
        cur.get("bsmod", 3)?;
        cur.get("copyrightb", 1)?;
        cur.get("origbs", 1)?;
        if acmod == 2 {
            cur.get("dsurmod", 2)?;
            cur.get("dheadphonmod", 2)?;
        }
        if acmod >= 6 {
            cur.get("dsurexmod", 2)?;
        }
        if cur.flag("audprodie")? {
            cur.get("mixlevel", 5)?;
            cur.get("roomtyp", 2)?;
            cur.get("adconvtyp", 1)?;
        }
        if acmod == 0 && cur.flag("audprodi2e")? {
            cur.get("mixlevel2", 5)?;
            cur.get("roomtyp2", 2)?;
            cur.get("adconvtyp2", 1)?;
        }
        if fscod < 3 {
            cur.get("sourcefscod", 1)?;
        }
    }
    if strmtyp == 0 && numblkscod != 3 {
        cur.get("convsync", 1)?;
    }
    if strmtyp == 2 {
        let blkid = if numblkscod == 3 {
            true
        } else {
            cur.flag("blkid")?
        };
        if blkid {
            cur.get("frmsizecod", 6)?;
        }
    }
    if cur.flag("addbsie")? {
        let l = cur.get("addbsil", 6)? as usize;
        cur.span("addbsi", Role::Addbsi, (l + 1) * 8)?;
    }

    // audfrm (E.1.2.3)
    let nfchans = NFCHANS[acmod as usize];
    let (expstre, ahte) = if numblkscod == 3 {
        (cur.flag("expstre")?, cur.flag("ahte")?)
    } else {
        (true, false)
    };
    let snroffststr = cur.get("snroffststr", 2)?;
    if snroffststr == 3 {
        return invalid("reserved snroffststr");
    }
    let transproce = cur.flag("transproce")?;
    let blkswe = cur.flag("blkswe")?;
    let dithflage = cur.flag("dithflage")?;
    let bamode = cur.flag("bamode")?;
    let frmfgaincode = cur.flag("frmfgaincode")?;
    let dbaflde = cur.flag("dbaflde")?;
    let skipflde = cur.flag("skipflde")?;
    let spxattene = cur.flag("spxattene")?;
    let mut cplstre = [false; 6];
    let mut cplinu = [false; 6];
    if acmod > 1 {
        cplstre[0] = true;
        cplinu[0] = cur.flagi("cplinu", 0)?;
        for blk in 1..numblks {
            cplstre[blk] = cur.flagi("cplstre", blk)?;
            cplinu[blk] = if cplstre[blk] {
                cur.flagi("cplinu", blk)?
            } else {
                cplinu[blk - 1]
            };
        }
    }
    let mut cplexpstr = [0u8; 6];
    let mut chexpstr = [[0u8; 5]; 6];
    let ncplblks = cplinu.iter().take(numblks).filter(|&&c| c).count();
    if expstre {
        for blk in 0..numblks {
            if cplinu[blk] {
                cplexpstr[blk] = cur.geti("cplexpstr", blk, 2)? as u8;
            }
            for ch in 0..nfchans {
                chexpstr[blk][ch] = cur.geti("chexpstr", blk, 2)? as u8;
            }
        }
    } else {
        if acmod > 1 && ncplblks > 0 {
            let f = cur.get("frmcplexpstr", 5)? as usize;
            cplexpstr.copy_from_slice(&FRAME_EXPSTR[f]);
        }
        for ch in 0..nfchans {
            let f = cur.geti("frmchexpstr", ch, 5)? as usize;
            for blk in 0..6 {
                chexpstr[blk][ch] = FRAME_EXPSTR[f][blk];
            }
        }
    }
    let mut lfeexpstr = [0u8; 6];
    if lfeon {
        for (blk, s) in lfeexpstr.iter_mut().enumerate().take(numblks) {
            *s = cur.geti("lfeexpstr", blk, 1)? as u8;
        }
    }
    if strmtyp == 0 {
        let convexpstre = if numblkscod != 3 {
            cur.flag("convexpstre")?
        } else {
            true
        };
        if convexpstre {
            for ch in 0..nfchans {
                cur.geti("convexpstr", ch, 5)?;
            }
        }
    }
    // AHT (E.2.4.2 helper variables).
    let mut ahtinu = [false; SLOTS];
    if ahte {
        let ncplregs = (0..6).filter(|&b| cplstre[b] || cplexpstr[b] != 0).count();
        if ncplblks == 6 && ncplregs == 1 {
            ahtinu[CPL] = cur.flag("cplahtinu")?;
        }
        for ch in 0..nfchans {
            let nchregs = (0..6).filter(|&b| chexpstr[b][ch] != 0).count();
            if nchregs == 1 {
                ahtinu[ch] = cur.flagi("chahtinu", ch)?;
            }
        }
        if lfeon {
            let nlferegs = (0..6).filter(|&b| lfeexpstr[b] != 0).count();
            if nlferegs == 1 {
                ahtinu[LFE] = cur.flag("lfeahtinu")?;
            }
        }
    }
    let mut st = AllocState::default();
    if snroffststr == 0 {
        let c = cur.get("frmcsnroffst", 6)? as u8;
        let f = cur.get("frmfsnroffst", 4)? as u8;
        st.csnroffst = Some(c);
        st.fsnroffst = [f; SLOTS];
    }
    if transproce {
        for ch in 0..nfchans {
            if cur.flagi("chintransproc", ch)? {
                cur.geti("transprocloc", ch, 10)?;
                cur.geti("transproclen", ch, 8)?;
            }
        }
    }
    if spxattene {
        for ch in 0..nfchans {
            if cur.flagi("chinspxatten", ch)? {
                cur.geti("spxattencod", ch, 5)?;
            }
        }
    }
    let blkstrtinfoe = if numblkscod != 0 {
        cur.get_role("blkstrtinfoe", Role::BlkStrtInfoE, -1, 1)? == 1
    } else {
        false
    };
    if blkstrtinfoe {
        let words = frmsiz + 1;
        let nbits = (numblks - 1) * (4 + ceil_log2(words));
        cur.span("blkstrtinfo", Role::BlkStrtInfo, nbits)?;
    }

    // audblk (E.1.2.4)
    let mut firstspxcos = [true; 5];
    let mut firstcplcos = [true; 5];
    let mut firstcplleak = true;
    let mut spxinu = false;
    let mut chinspx = [false; 5];
    let mut spx_begin = 0usize;
    let mut spx_end = 0usize;
    let mut spxbegf = 0usize;
    let mut spxbndstrc = DEF_SPX_BNDSTRC;
    let mut chincpl = [false; 5];
    let mut phsflginu = false;
    let mut cplbegf = 0usize;
    let mut cplendf: i32 = 0;
    let mut cplbndstrc = DEF_CPL_BNDSTRC;
    let mut ncplbnd = 0usize;
    let mut exps: [Option<ExpSet>; SLOTS] = Default::default();
    let mut chbwcod = [None::<u32>; 5];
    let mut aht_read = [false; SLOTS];
    let mut hebap: [Vec<u8>; SLOTS] = Default::default();
    let mut block_starts = Vec::with_capacity(numblks);
    let mut mantissa_starts = Vec::with_capacity(numblks);
    let mut aht_tags = 0;
    let mut spx_blocks = 0;
    let mut cpl_blocks = 0;

    for blk in 0..numblks {
        cur.block = blk as i8;
        block_starts.push(cur.pos);
        if blkswe {
            for ch in 0..nfchans {
                cur.geti("blksw", ch, 1)?;
            }
        }
        if dithflage {
            for ch in 0..nfchans {
                cur.geti("dithflag", ch, 1)?;
            }
        }
        if cur.get_role("dynrnge", Role::DynrngExists(0), -1, 1)? == 1 {
            cur.get_role("dynrng", Role::Dynrng(0), -1, 8)?;
        }
        if acmod == 0 && cur.get_role("dynrng2e", Role::DynrngExists(1), -1, 1)? == 1 {
            cur.get_role("dynrng2", Role::Dynrng(1), -1, 8)?;
        }
        // Spectral extension strategy.
        let spxstre = if blk == 0 { true } else { cur.flag("spxstre")? };
        if spxstre {
            spxinu = cur.flag("spxinu")?;
            if spxinu {
                if acmod == 1 {
                    chinspx[0] = true;
                } else {
                    for (ch, c) in chinspx.iter_mut().enumerate().take(nfchans) {
                        *c = cur.flagi("chinspx", ch)?;
                    }
                }
                cur.get("spxstrtf", 2)?;
                spxbegf = cur.get("spxbegf", 3)? as usize;
                let spxendf = cur.get("spxendf", 3)? as usize;
                spx_begin = if spxbegf < 6 {
                    spxbegf + 2
                } else {
                    spxbegf * 2 - 3
                };
                spx_end = if spxendf < 3 {
                    spxendf + 5
                } else {
                    spxendf * 2 + 3
                };
                if spx_begin >= spx_end {
                    return invalid("spectral extension begins at or after its end");
                }
                if cur.flag("spxbndstrce")? {
                    for bnd in spx_begin + 1..spx_end {
                        spxbndstrc[bnd] = cur.flagi("spxbndstrc", bnd)?;
                    }
                }
            } else {
                chinspx = [false; 5];
                firstspxcos = [true; 5];
            }
        }
        if spxinu {
            spx_blocks += 1;
            let nspxbnds = 1 + (spx_begin + 1..spx_end).filter(|&b| !spxbndstrc[b]).count();
            for ch in 0..nfchans {
                if chinspx[ch] {
                    let spxcoe = if firstspxcos[ch] {
                        firstspxcos[ch] = false;
                        true
                    } else {
                        cur.flagi("spxcoe", ch)?
                    };
                    if spxcoe {
                        cur.geti("spxblnd", ch, 5)?;
                        cur.geti("mstrspxco", ch, 2)?;
                        for _ in 0..nspxbnds {
                            cur.geti("spxcoexp", ch, 4)?;
                            cur.geti("spxcomant", ch, 2)?;
                        }
                    }
                } else {
                    firstspxcos[ch] = true;
                }
            }
        }
        // Coupling strategy.
        if cplstre[blk] {
            if cplinu[blk] {
                if cur.flag("ecplinu")? {
                    return refuse("enhanced coupling");
                }
                if acmod == 2 {
                    chincpl[0] = true;
                    chincpl[1] = true;
                } else {
                    for (ch, c) in chincpl.iter_mut().enumerate().take(nfchans) {
                        *c = cur.flagi("chincpl", ch)?;
                    }
                }
                phsflginu = if acmod == 2 {
                    cur.flag("phsflginu")?
                } else {
                    false
                };
                cplbegf = cur.get("cplbegf", 4)? as usize;
                cplendf = if !spxinu {
                    cur.get("cplendf", 4)? as i32
                } else if spxbegf < 6 {
                    spxbegf as i32 - 2
                } else {
                    spxbegf as i32 * 2 - 7
                };
                let ncplsubnd = 3 + cplendf - cplbegf as i32;
                if ncplsubnd < 1 || cplbegf as i32 + ncplsubnd > 18 {
                    return invalid("coupling sub-band range");
                }
                let ncplsubnd = ncplsubnd as usize;
                if cur.flag("cplbndstrce")? {
                    for bnd in 1..ncplsubnd {
                        cplbndstrc[cplbegf + bnd] = cur.flagi("cplbndstrc", bnd)?;
                    }
                }
                ncplbnd = 1
                    + (cplbegf + 1..cplbegf + ncplsubnd)
                        .filter(|&b| !cplbndstrc[b])
                        .count();
            } else {
                chincpl = [false; 5];
                firstcplcos = [true; 5];
                firstcplleak = true;
                phsflginu = false;
            }
        }
        if cplinu[blk] {
            cpl_blocks += 1;
            let mut cplcoe = [false; 5];
            for ch in 0..nfchans {
                if chincpl[ch] {
                    cplcoe[ch] = if firstcplcos[ch] {
                        firstcplcos[ch] = false;
                        true
                    } else {
                        cur.flagi("cplcoe", ch)?
                    };
                    if cplcoe[ch] {
                        cur.geti("mstrcplco", ch, 2)?;
                        for _ in 0..ncplbnd {
                            cur.geti("cplcoexp", ch, 4)?;
                            cur.geti("cplcomant", ch, 4)?;
                        }
                    }
                } else {
                    firstcplcos[ch] = true;
                }
            }
            if acmod == 2 && phsflginu && (cplcoe[0] || cplcoe[1]) {
                for bnd in 0..ncplbnd {
                    cur.geti("phsflg", bnd, 1)?;
                }
            }
        }
        // Rematrixing (E.2.3.2).
        if acmod == 2 {
            let rematstr = if blk == 0 {
                true
            } else {
                cur.flag("rematstr")?
            };
            if rematstr {
                let n = if cplinu[blk] {
                    if cplbegf == 0 {
                        2
                    } else if cplbegf < 3 {
                        3
                    } else {
                        4
                    }
                } else if spxinu {
                    if spxbegf < 2 {
                        3
                    } else {
                        4
                    }
                } else {
                    4
                };
                for bnd in 0..n {
                    cur.geti("rematflg", bnd, 1)?;
                }
            }
        }
        // Channel bandwidth.
        for ch in 0..nfchans {
            if chexpstr[blk][ch] != 0 && !chincpl[ch] && !chinspx[ch] {
                let c = cur.geti("chbwcod", ch, 6)?;
                if c > 60 {
                    return invalid(format!("chbwcod {c} above 60"));
                }
                chbwcod[ch] = Some(c);
            }
        }
        let cplstrt = 37 + 12 * cplbegf;
        let cplend = (37 + 12 * (cplendf + 3)) as usize;
        let spxstrt = 25 + 12 * spx_begin;
        let mut endmant = [0usize; 5];
        for ch in 0..nfchans {
            endmant[ch] = if cplinu[blk] && chincpl[ch] {
                cplstrt
            } else if spxinu && chinspx[ch] {
                spxstrt
            } else {
                match chbwcod[ch] {
                    Some(c) => 37 + 3 * (c as usize + 12),
                    None => return invalid("channel bandwidth unknown"),
                }
            };
        }
        // Exponents.
        if cplinu[blk] {
            if cplexpstr[blk] != 0 {
                exps[CPL] = Some(read_exponents(
                    &mut cur,
                    CPL,
                    cplexpstr[blk],
                    cplstrt,
                    cplend,
                )?);
            } else if exps[CPL]
                .as_ref()
                .is_none_or(|e| e.start != cplstrt || e.end != cplend)
            {
                return invalid("coupling exponent reuse across a changed range");
            }
        }
        for ch in 0..nfchans {
            if chexpstr[blk][ch] != 0 {
                exps[ch] = Some(read_exponents(
                    &mut cur,
                    ch,
                    chexpstr[blk][ch],
                    0,
                    endmant[ch],
                )?);
                cur.geti("gainrng", ch, 2)?;
            } else if exps[ch].as_ref().is_none_or(|e| e.end != endmant[ch]) {
                return invalid("channel exponent reuse across a changed range");
            }
        }
        if lfeon {
            if lfeexpstr[blk] != 0 {
                exps[LFE] = Some(read_exponents(&mut cur, LFE, 1, 0, 7)?);
            } else if exps[LFE].is_none() {
                return invalid("LFE exponent reuse without exponents");
            }
        }
        // Bit allocation parametric information.
        if bamode {
            if cur.flag("baie")? {
                st.bai = Some([
                    cur.get("sdcycod", 2)? as u8,
                    cur.get("fdcycod", 2)? as u8,
                    cur.get("sgaincod", 2)? as u8,
                    cur.get("dbpbcod", 2)? as u8,
                    cur.get("floorcod", 3)? as u8,
                ]);
            }
        } else {
            st.bai = Some([2, 1, 1, 2, 7]);
        }
        if snroffststr != 0 {
            let snroffste = if blk == 0 {
                true
            } else {
                cur.flag("snroffste")?
            };
            if snroffste {
                st.csnroffst = Some(cur.get("csnroffst", 6)? as u8);
                if snroffststr == 1 {
                    let f = cur.get("blkfsnroffst", 4)? as u8;
                    st.fsnroffst = [f; SLOTS];
                } else {
                    if cplinu[blk] {
                        st.fsnroffst[CPL] = cur.get("cplfsnroffst", 4)? as u8;
                    }
                    for ch in 0..nfchans {
                        st.fsnroffst[ch] = cur.geti("fsnroffst", ch, 4)? as u8;
                    }
                    if lfeon {
                        st.fsnroffst[LFE] = cur.get("lfefsnroffst", 4)? as u8;
                    }
                }
            }
        }
        let fgaincode = if frmfgaincode {
            cur.flag("fgaincode")?
        } else {
            false
        };
        if fgaincode {
            if cplinu[blk] {
                st.fgaincod[CPL] = cur.get("cplfgaincod", 3)? as u8;
            }
            for ch in 0..nfchans {
                st.fgaincod[ch] = cur.geti("fgaincod", ch, 3)? as u8;
            }
            if lfeon {
                st.fgaincod[LFE] = cur.get("lfefgaincod", 3)? as u8;
            }
        } else {
            st.fgaincod = [4; SLOTS];
        }
        if strmtyp == 0 && cur.flag("convsnroffste")? {
            cur.get("convsnroffst", 10)?;
        }
        if cplinu[blk] {
            let cplleake = if firstcplleak {
                firstcplleak = false;
                true
            } else {
                cur.flag("cplleake")?
            };
            if cplleake {
                st.cplleak = Some((cur.get("cplfleak", 3)? as u8, cur.get("cplsleak", 3)? as u8));
            }
        }
        // Delta bit allocation.
        if dbaflde {
            if cur.flag("deltbaie")? {
                let mut codes = [0u32; SLOTS];
                if cplinu[blk] {
                    codes[CPL] = cur.get("cpldeltbae", 2)?;
                    st.delta_code(CPL, codes[CPL], blk == 0)?;
                }
                for ch in 0..nfchans {
                    codes[ch] = cur.geti("deltbae", ch, 2)?;
                    st.delta_code(ch, codes[ch], blk == 0)?;
                }
                if cplinu[blk] && codes[CPL] == 1 {
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
        }
        // Skip field.
        if skipflde && cur.get_role("skiple", Role::Skiple, -1, 1)? == 1 {
            let l = cur.get_role("skipl", Role::Skipl, -1, 9)? as usize;
            cur.span("skipfld", Role::Skipfld, 8 * l)?;
        }
        // Bit allocation for this block. AHT slots are allocated once, with hebap, in the
        // block where their (only) exponent set is transmitted.
        let mut req = Vec::with_capacity(SLOTS);
        for ch in 0..nfchans {
            if ahtinu[ch] && aht_read[ch] {
                continue;
            }
            req.push(SlotAlloc {
                slot: ch,
                exps: exps[ch].as_ref().expect("checked above"),
                start: 0,
                end: endmant[ch],
                high_efficiency: ahtinu[ch],
            });
        }
        if cplinu[blk] && !(ahtinu[CPL] && aht_read[CPL]) {
            req.push(SlotAlloc {
                slot: CPL,
                exps: exps[CPL].as_ref().expect("checked above"),
                start: cplstrt,
                end: cplend,
                high_efficiency: ahtinu[CPL],
            });
        }
        if lfeon && !(ahtinu[LFE] && aht_read[LFE]) {
            req.push(SlotAlloc {
                slot: LFE,
                exps: exps[LFE].as_ref().expect("checked above"),
                start: 0,
                end: 7,
                high_efficiency: ahtinu[LFE],
            });
        }
        let mut bap = allocate_block(&st, fscod as usize, &req)?;
        for s in 0..SLOTS {
            if ahtinu[s] && !aht_read[s] && !bap[s].is_empty() {
                hebap[s] = std::mem::take(&mut bap[s]);
            }
        }
        // Mantissas.
        let start = cur.pos;
        mantissa_starts.push(start);
        let mut g = Groups::default();
        let mut got_cpl = false;
        for ch in 0..nfchans {
            if !ahtinu[ch] {
                conventional(&mut cur, &bap[ch], 0, endmant[ch], &mut g)?;
            } else if !aht_read[ch] {
                aht_tags += aht(&mut cur, &hebap[ch], 0, endmant[ch])?;
                aht_read[ch] = true;
            }
            if cplinu[blk] && chincpl[ch] && !got_cpl {
                if !ahtinu[CPL] {
                    conventional(&mut cur, &bap[CPL], cplstrt, cplend, &mut g)?;
                } else if !aht_read[CPL] {
                    aht_tags += aht(&mut cur, &hebap[CPL], cplstrt, cplend)?;
                    aht_read[CPL] = true;
                }
                got_cpl = true;
            }
        }
        if lfeon {
            if !ahtinu[LFE] {
                conventional(&mut cur, &bap[LFE], 0, 7, &mut g)?;
            } else if !aht_read[LFE] {
                aht_tags += aht(&mut cur, &hebap[LFE], 0, 7)?;
                aht_read[LFE] = true;
            }
        }
        cur.close_span("mantissas", Role::Mantissas, start);
    }
    let audio_end = cur.pos;
    read_tail(&mut cur, bytes * 8, Codec::Eac3)?;
    Ok(Frame {
        codec: Codec::Eac3,
        bytes,
        fields: cur.fields,
        bsid,
        acmod,
        lfeon,
        fscod,
        strmtyp,
        substreamid,
        numblks,
        block_starts,
        mantissa_starts,
        audio_end,
        aht_tags,
        aht_slots: (0..SLOTS).filter(|&s| ahtinu[s]).collect(),
        spx_blocks,
        cpl_blocks,
    })
}

fn ceil_log2(n: usize) -> usize {
    let mut b = 0;
    while (1usize << b) < n {
        b += 1;
    }
    b
}
