//! ddpmeta command line: `show`, `strip` and the `fields` diagnostic.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ddpmeta::edit::{Method, Strip};
use ddpmeta::emdf_protection::{load_keys, KeySet};
use ddpmeta::error::Error;
use ddpmeta::show::show_to;
use ddpmeta::stream::{parse_frame, FrameReader};
use ddpmeta::strip::{strip_in_place, strip_stream, Options, Report};

const IO_BUFFER: usize = 1 << 16;

fn help() -> String {
    let mut h = format!(
        "ddpmeta {}
Show and strip DRC and dialnorm metadata in DD (AC-3) and DD+ (E-AC-3) streams.

Usage:
  ddpmeta show [options] IN
  ddpmeta strip [options] (-o OUT | --in-place) IN
  ddpmeta fields IN FRAME

Commands:
  show    print a summary per substream: dialnorm, DRC statistics, EMDF
  strip   remove DRC or set dialnorm, keeping EMDF protection valid
  fields  print every parsed element of syncframe FRAME (counting from 0)

IN can be - for standard input, and OUT - for standard output.

Show options:
  -v, --verbose             also list every syncframe and EMDF container

Strip options (at least one of --line, --rf, --dialnorm):
  --line                    remove line mode DRC (dynrng)
  --rf                      neutralize RF mode DRC (compr)
  --dialnorm                set dialnorm to -31 dB, and the EMDF loudness to match
  --method a|b              a: overwrite the DRC words (default); b: delete the dynrng words
  -o OUT                    write to OUT, only if every syncframe succeeds
  --in-place                edit IN itself (method a only), only if every syncframe succeeds
  --allow-stale-protection  write EMDF containers whose protection no longer verifies
",
        env!("CARGO_PKG_VERSION")
    );
    if !cfg!(feature = "embed-key") {
        h.push_str(
            "
Key options (show and strip):
  --emdf-key-file PATH      read the EMDF keys from PATH, not from EMDF_PROTECTION_KEY_FILE;
                            the binary key set, or one key in hex used as key_id 0
",
        );
    }
    h.push_str(
        "
General options:
  -h, --help                print this help
  -V, --version             print the version
",
    );
    h
}

/// Every option: its name, whether it takes a value, and the commands it applies to.
const OPTIONS: &[(&str, bool, &[&str])] = &[
    ("-v", false, &["show"]),
    ("--verbose", false, &["show"]),
    ("--emdf-key-file", true, &["show", "strip"]),
    ("--line", false, &["strip"]),
    ("--rf", false, &["strip"]),
    ("--dialnorm", false, &["strip"]),
    ("--method", true, &["strip"]),
    ("-o", true, &["strip"]),
    ("--in-place", false, &["strip"]),
    ("--allow-stale-protection", false, &["strip"]),
];

#[derive(Default)]
struct Cli {
    verbose: bool,
    key_file: Option<String>,
    strip: Strip,
    method: Option<Method>,
    out: Option<String>,
    in_place: bool,
    allow_stale: bool,
    pos: Vec<String>,
}

/// Parse the options of `cmd`. Errors name the offending argument.
fn parse(cmd: &str, args: &[String]) -> Result<Cli, String> {
    let mut c = Cli::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            c.pos.extend(it.by_ref().cloned());
            break;
        }
        if a == "-" || !a.starts_with('-') {
            c.pos.push(a.clone());
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let Some(&(_, takes_value, cmds)) = OPTIONS.iter().find(|o| o.0 == name) else {
            return Err(format!("unknown option {name}"));
        };
        if !cmds.contains(&cmd) {
            return Err(format!("{name} does not apply to {cmd}"));
        }
        let value = if takes_value {
            match inline.or_else(|| it.next().cloned()) {
                Some(v) => Some(v),
                None => return Err(format!("{name} needs a value")),
            }
        } else if inline.is_some() {
            return Err(format!("{name} takes no value"));
        } else {
            None
        };
        match name {
            "-v" | "--verbose" => c.verbose = true,
            "--line" => c.strip.line = true,
            "--rf" => c.strip.rf = true,
            "--dialnorm" => c.strip.dialnorm = true,
            "--in-place" => c.in_place = true,
            "--allow-stale-protection" => c.allow_stale = true,
            "--emdf-key-file" => {
                once(name, &c.key_file)?;
                c.key_file = value;
            }
            "-o" => {
                once(name, &c.out)?;
                c.out = value;
            }
            "--method" => {
                once(name, &c.method)?;
                c.method = Some(match value.as_deref() {
                    Some("a" | "A") => Method::InPlace,
                    Some("b" | "B") => Method::Repack,
                    _ => return Err("--method takes a or b".into()),
                });
            }
            _ => unreachable!("every option in OPTIONS is handled"),
        }
    }
    Ok(c)
}

fn once<T>(name: &str, slot: &Option<T>) -> Result<(), String> {
    match slot {
        Some(_) => Err(format!("{name} given twice")),
        None => Ok(()),
    }
}

/// Why a command failed: a usage error (exit status 2) or a failure while running (1).
enum Fail {
    Usage(String),
    Run(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Run(e)
    }
}

fn open_input(path: &str) -> Result<Box<dyn Read>, Error> {
    if path == "-" {
        return Ok(Box::new(BufReader::with_capacity(
            IO_BUFFER,
            io::stdin().lock(),
        )));
    }
    let f = File::open(path).map_err(|e| Error::Io(format!("{path}: {e}")))?;
    Ok(Box::new(BufReader::with_capacity(IO_BUFFER, f)))
}

/// True when both paths name the same existing file, through links and relative paths too.
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// True when both paths name the same existing file. Without file ids in stable std this
/// compares canonical paths, which misses hard links.
#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Standard output for reports: a reader that stops early (`| head`) ends the program quietly.
struct Quiet<W>(W);

impl<W: Write> Write for Quiet<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self.0.write(b) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
            r => r,
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.flush() {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => std::process::exit(0),
            r => r,
        }
    }
}

fn report_out() -> Quiet<BufWriter<io::StdoutLock<'static>>> {
    Quiet(BufWriter::with_capacity(IO_BUFFER, io::stdout().lock()))
}

fn keys(c: &Cli) -> Result<Option<KeySet>, Error> {
    load_keys(c.key_file.as_deref())
}

fn show(c: &Cli) -> Result<(), Fail> {
    let [input] = c.pos.as_slice() else {
        return Err(Fail::Usage(format!(
            "show needs one IN, got {}",
            c.pos.len()
        )));
    };
    let keys = keys(c)?;
    let r = open_input(input)?;
    let mut out = report_out();
    if let Some(k) = &keys {
        let ids: Vec<String> = k.key_ids().map(|i| i.to_string()).collect();
        let _ = writeln!(
            out,
            "EMDF keys: configured, {} (key_id {})",
            k.len(),
            ids.join(", ")
        );
    }
    Ok(show_to(r, keys.as_ref(), c.verbose, &mut out)?)
}

fn fields(c: &Cli) -> Result<(), Fail> {
    let [input, frame] = c.pos.as_slice() else {
        return Err(Fail::Usage("fields needs IN and FRAME".into()));
    };
    let want: usize = frame
        .parse()
        .map_err(|_| Fail::Usage(format!("FRAME must be a number, not {frame}")))?;
    let mut frames = FrameReader::new(open_input(input)?);
    let mut count = 0;
    while let Some(fr) = frames.next_frame()? {
        count += 1;
        if fr.index != want {
            continue;
        }
        let f = parse_frame(fr.data)?;
        let mut out = report_out();
        for fl in &f.fields {
            let _ = writeln!(
                out,
                "{:6} {:5} {:>16} block {:2} index {:3} role {:?} = {}",
                fl.pos, fl.width, fl.name, fl.block, fl.index, fl.role, fl.value
            );
        }
        return out.flush().map_err(|e| Fail::Run(Error::Io(e.to_string())));
    }
    Err(Fail::Run(Error::Io(format!(
        "the stream has only {count} frames (FRAME counts from 0)"
    ))))
}

/// Where `strip` writes, decided before any input is read.
enum Target {
    Stdout,
    /// A regular file, written through a temporary file beside it that replaces it only once
    /// every syncframe has succeeded.
    File(PathBuf),
    /// Something that is not a regular file, such as a device or a named pipe: written
    /// directly.
    Special(PathBuf),
    InPlace(PathBuf),
}

fn target(c: &Cli, input: &str) -> Result<Target, String> {
    match (&c.out, c.in_place) {
        (Some(_), true) => Err("-o and --in-place cannot be combined".into()),
        (None, false) => Err("strip needs -o OUT or --in-place".into()),
        (None, true) if input == "-" => Err("--in-place needs a file, not standard input".into()),
        (None, true) if c.method == Some(Method::Repack) => Err(
            "--in-place needs method a: method b rewrites most of every frame, so use -o".into(),
        ),
        (None, true) => Ok(Target::InPlace(input.into())),
        (Some(o), false) if o == "-" => {
            if io::stdout().is_terminal() {
                Err("OUT is a terminal; redirect standard output or give -o FILE".into())
            } else {
                Ok(Target::Stdout)
            }
        }
        (Some(o), false) => {
            let o = PathBuf::from(o);
            if input != "-" && same_file(Path::new(input), &o) {
                return Err("OUT is the input file; use --in-place to edit it".into());
            }
            match std::fs::metadata(&o) {
                Ok(m) if !m.is_file() => Ok(Target::Special(o)),
                _ => Ok(Target::File(o)),
            }
        }
    }
}

/// Write the stripped stream to a temporary file beside `out`, then move it over `out`.
fn strip_to_file(r: impl Read, out: &Path, opt: &Options) -> Result<Report, Error> {
    let dir = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = out
        .file_name()
        .map_or("out".into(), |n| n.to_string_lossy());
    let tmp = dir.join(format!(".{name}.ddpmeta-{}.tmp", std::process::id()));
    let io = |e: io::Error| Error::Io(format!("{}: {e}", tmp.display()));
    let f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(io)?;
    let mut w = BufWriter::with_capacity(IO_BUFFER, f);
    let done = strip_stream(r, &mut w, opt).and_then(|rep| {
        let f = w.into_inner().map_err(|e| io(e.into_error()))?;
        f.sync_all().map_err(io)?;
        std::fs::rename(&tmp, out).map_err(io)?;
        Ok(rep)
    });
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

fn strip(c: &Cli) -> Result<(), Fail> {
    let [input] = c.pos.as_slice() else {
        return Err(Fail::Usage(format!(
            "strip needs one IN, got {}",
            c.pos.len()
        )));
    };
    if !(c.strip.line || c.strip.rf || c.strip.dialnorm) {
        return Err(Fail::Usage(
            "strip needs at least one of --line, --rf, --dialnorm".into(),
        ));
    }
    let target = target(c, input).map_err(Fail::Usage)?;
    let opt = Options {
        strip: c.strip,
        method: c.method.unwrap_or(Method::InPlace),
        key: keys(c)?,
        allow_stale_protection: c.allow_stale,
    };
    let rep = match &target {
        Target::InPlace(p) => strip_in_place(p, &opt)?,
        Target::File(p) => strip_to_file(open_input(input)?, p, &opt)?,
        Target::Special(p) => {
            let f = OpenOptions::new()
                .write(true)
                .open(p)
                .map_err(|e| Error::Io(format!("{}: {e}", p.display())))?;
            strip_stream(open_input(input)?, &mut BufWriter::new(f), &opt)?
        }
        Target::Stdout => strip_stream(
            open_input(input)?,
            &mut BufWriter::with_capacity(IO_BUFFER, io::stdout().lock()),
            &opt,
        )?,
    };
    let mut text = format!(
        "{} frames, {} changed: {} gain words set to 0 dB, {} gain words removed ({} bits moved to auxbits), {} dialnorm words set to 31, {} loudness references set to -31 LUFS; EMDF containers re-signed {}, left stale {}\n",
        rep.frames, rep.frames_changed, rep.words_zeroed, rep.words_removed, rep.freed_bits,
        rep.dialnorm_set, rep.loudness_set, rep.containers_resigned, rep.containers_stale
    );
    for n in &rep.notes {
        text.push_str(&format!("note: {n}\n"));
    }
    // With the stream on standard output, the report goes to standard error.
    if matches!(target, Target::Stdout) {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
    Ok(())
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("ddpmeta: {msg}\nRun 'ddpmeta --help' for usage.");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let before_dashes = args.iter().take_while(|a| *a != "--");
    for a in before_dashes {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{}", help());
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("ddpmeta {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ => {}
        }
    }
    let Some(cmd) = args.first() else {
        eprint!("{}", help());
        return ExitCode::from(2);
    };
    if cmd == "help" {
        print!("{}", help());
        return ExitCode::SUCCESS;
    }
    if !["show", "strip", "fields"].contains(&cmd.as_str()) {
        return usage_error(&format!("unknown command {cmd}"));
    }
    let c = match parse(cmd, &args[1..]) {
        Ok(c) => c,
        Err(e) => return usage_error(&e),
    };
    let done = match cmd.as_str() {
        "show" => show(&c),
        "strip" => strip(&c),
        _ => fields(&c),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fail::Usage(e)) => usage_error(&e),
        Err(Fail::Run(e)) => {
            eprintln!("ddpmeta: {e}");
            ExitCode::from(1)
        }
    }
}
