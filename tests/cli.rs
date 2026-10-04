//! The command line: help, usage errors and their exit status, `-o` against `--in-place`, and
//! standard input and output.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ddpmeta");

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

/// A fresh directory for one test.
fn scratch(test: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The binary, without a key file from the environment, so output does not depend on it.
fn ddpmeta() -> Command {
    let mut c = Command::new(BIN);
    c.env_remove("EMDF_PROTECTION_KEY_FILE");
    c
}

fn run(args: &[&str]) -> Output {
    ddpmeta().args(args).stdin(Stdio::null()).output().unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn help_and_version_succeed() {
    for args in [&["--help"][..], &["-h"], &["help"], &["show", "--help"]] {
        let o = run(args);
        assert!(o.status.success(), "{args:?}");
        assert!(text(&o.stdout).contains("Usage:"), "{args:?}");
    }
    let o = run(&["--version"]);
    assert!(o.status.success());
    assert_eq!(
        text(&o.stdout),
        format!("ddpmeta {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn usage_errors_exit_2_and_name_the_problem() {
    let d = scratch("usage_errors");
    let input = data("dd51drc.ac3");
    let i = input.to_str().unwrap();
    let copy = d.join("in.ac3");
    std::fs::copy(&input, &copy).unwrap();
    // The same file through another spelling of its path.
    let alias = d.join(".").join("in.ac3");
    let cases: &[(&[&str], &str)] = &[
        (&["shwo", i], "unknown command shwo"),
        (&["show", "--bogus", i], "unknown option --bogus"),
        (&["show", "--line", i], "--line does not apply to show"),
        (
            &["show", i, "--emdf-key-file"],
            "--emdf-key-file needs a value",
        ),
        (&["show", "-v=1", i], "unknown option -v=1"),
        (&["show", "--verbose=yes", i], "--verbose takes no value"),
        (&["show"], "show needs one IN, got 0"),
        (&["strip", "-o", "x", i], "strip needs at least one of"),
        (&["strip", "--line", i], "strip needs -o OUT or --in-place"),
        (
            &["strip", "--line", "-o", "x", "--in-place", i],
            "cannot be combined",
        ),
        (
            &["strip", "--line", "--in-place", "-"],
            "--in-place needs a file",
        ),
        (
            &["strip", "--line", "--method", "b", "--in-place", i],
            "needs method a",
        ),
        (
            &["strip", "--line", "--method", "c", "-o", "x", i],
            "--method takes a or b",
        ),
        (
            &["strip", "--line", "-o", "x", "-o", "y", i],
            "-o given twice",
        ),
        (
            &[
                "strip",
                "--line",
                "-o",
                alias.to_str().unwrap(),
                copy.to_str().unwrap(),
            ],
            "OUT is the input file",
        ),
        (&["fields", i, "x"], "FRAME must be a number"),
    ];
    for (args, want) in cases {
        let o = run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}: {}", text(&o.stderr));
        assert!(
            text(&o.stderr).contains(want),
            "{args:?}: {}",
            text(&o.stderr)
        );
    }
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(&input).unwrap()
    );
}

/// `--in-place` (method A, which changes no frame's size) gives the bytes `-o` writes, and the
/// same report.
#[test]
fn in_place_matches_the_output_file() {
    let d = scratch("in_place");
    for name in ["dd51drc.ac3", "ddp51drc.ec3", "bd71drc.ec3"] {
        let input = data(name);
        let out = d.join(format!("out-{name}"));
        let edit = d.join(format!("edit-{name}"));
        std::fs::copy(&input, &edit).unwrap();
        let flags = ["--line", "--rf", "--dialnorm", "--allow-stale-protection"];
        let o1 = run(&[
            &["strip"][..],
            &flags,
            &["-o", out.to_str().unwrap(), input.to_str().unwrap()],
        ]
        .concat());
        let o2 = run(&[
            &["strip"][..],
            &flags,
            &["--in-place", edit.to_str().unwrap()],
        ]
        .concat());
        assert!(
            o1.status.success() && o2.status.success(),
            "{name}: {}",
            text(&o2.stderr)
        );
        assert_eq!(o1.stdout, o2.stdout, "{name}");
        let edited = std::fs::read(&edit).unwrap();
        assert_eq!(edited, std::fs::read(&out).unwrap(), "{name}");
        assert_ne!(edited, std::fs::read(&input).unwrap(), "{name}");
    }
}

/// A refused strip leaves the input untouched under `--in-place`, and writes no OUT (nor a
/// temporary file) under `-o`.
#[test]
fn refused_strip_writes_nothing() {
    let d = scratch("refused");
    let input = data("ddp51.ec3"); // protected EMDF containers, no key configured
    let edit = d.join("edit.ec3");
    std::fs::copy(&input, &edit).unwrap();
    let o = run(&["strip", "--line", "--in-place", edit.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o.stderr).contains("no key is configured"),
        "{}",
        text(&o.stderr)
    );
    assert_eq!(
        std::fs::read(&edit).unwrap(),
        std::fs::read(&input).unwrap()
    );
    let out = d.join("out.ec3");
    let o = run(&[
        "strip",
        "--line",
        "-o",
        out.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    let left: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["edit.ec3"]);
}

/// `-` reads standard input and writes standard output; the strip report then goes to
/// standard error so it cannot mix with the stream.
#[test]
fn pipes() {
    let d = scratch("pipes");
    let input = data("dd51drc.ac3");
    let out = d.join("out.ac3");
    let o = run(&[
        "strip",
        "--line",
        "-o",
        out.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert!(o.status.success());
    let piped = ddpmeta()
        .args(["strip", "--line", "-o", "-", "-"])
        .stdin(std::fs::File::open(&input).unwrap())
        .output()
        .unwrap();
    assert!(piped.status.success(), "{}", text(&piped.stderr));
    assert_eq!(piped.stdout, std::fs::read(&out).unwrap());
    assert_eq!(text(&piped.stderr), text(&o.stdout));
    let shown = ddpmeta()
        .args(["show", "-"])
        .stdin(std::fs::File::open(&input).unwrap())
        .output()
        .unwrap();
    assert_eq!(
        text(&shown.stdout),
        text(&run(&["show", input.to_str().unwrap()]).stdout)
    );
}
