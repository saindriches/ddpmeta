# ddpmeta

Shows and strips DRC and dialogue normalization metadata in DD (AC-3) and DD+ (E-AC-3)
streams, without re-encoding. DD+ JOC and the Blu-ray profile (an AC-3 core with an E-AC-3
dependent substream) are included. Parsing follows ETSI TS 102 366 V1.4.1.

ddpmeta is an independent tool, not affiliated with or endorsed by Dolby Laboratories. Dolby,
Dolby Digital and Dolby Digital Plus are trademarks of Dolby Laboratories.

## Usage

```
ddpmeta show [options] IN
ddpmeta strip [options] (-o OUT | --in-place) IN
ddpmeta fields IN FRAME
```

`ddpmeta --help` lists every option. IN and OUT can be `-` for standard input and output.
Streams are processed one syncframe at a time, so memory use stays small at any length.

### show

Prints a summary per substream: dialnorm, DRC statistics for line mode (dynrng) and RF mode
(compr), and the EMDF containers. The statistics are weighted by time: the share of time cut or
boosted by more than 1 dB, gain quantiles, and a histogram. `-v` also lists every syncframe.

```
E-AC-3 independent substream 0: 88 frames, 2.82 s
  dialnorm: -31 dB in every frame
  dynrng (line mode): cut over 1 dB 30.9%, within 1 dB 37.1%, boost over 1 dB 32.0% of the time
    gain dB: min -2.50, p10 -1.97, median -0.14, p90 +3.15, max +3.52, mean +0.23
     -3 to -1 dB  #########                       30.9%
      -1 to 0 dB  ######                          21.2%
            0 dB  #                                4.4%
      0 to +1 dB  ###                             11.6%
     +1 to +3 dB  ######                          19.3%
     +3 to +6 dB  ####                            12.7%
```

### strip

Edits every substream and recomputes every CRC:

* `--line` removes line mode DRC: dynrng is set to 0 dB.
* `--rf` neutralizes RF mode DRC. compr is kept, set to a near 0 dB code, since decoders play a
  frame without compr about 11 dB quieter in RF mode.
* `--dialnorm` sets dialnorm to -31 dB, and the EMDF programme loudness to match.
* `--method a` (default) overwrites the words; `--method b` deletes the dynrng words and pads
  auxbits, so the frame size stays the same.

`-o FILE` writes nothing unless every syncframe succeeds. `--in-place` edits IN itself (method a
only), and likewise writes nothing until every syncframe has succeeded. On standard output, a
failure leaves the frames written before it.

strip refuses anything it cannot validate, including an edit that would leave EMDF protection
invalid. `--allow-stale-protection` writes such containers anyway, but some players then stop
decoding.

### fields

Prints every parsed element of one syncframe, counting from 0.

## Build

```
cargo build --release
cargo test --release
```

Rust 1.87 or newer, no dependencies. See `AGENTS.md` before sending a change.

## License

Apache License 2.0; see `LICENSE`.
