# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). The release workflow takes
the release notes from here (`.github/release-notes.sh`).

## [Unreleased]

## [0.1.0] - 2026-10-05

### Added
- `show` summarizes each substream: dialnorm, line and RF mode DRC statistics (time shares, quantiles, histogram) and the EMDF protection status. `-v` lists every syncframe and container too.
- `strip` removes DRC for line mode (`--line`), neutralizes it for RF mode (`--rf`) and sets dialnorm to -31 dB (`--dialnorm`). `--method b` deletes the dynrng words instead of zeroing them, and every CRC is recomputed.
- `fields` dumps every parsed element of one syncframe.
- `strip --in-place` edits a file without a second copy (method a). `-` reads standard input or writes standard output, and streams are processed a frame at a time, so memory stays small at any length.
- Reads AC-3, E-AC-3, DD+ JOC and the Blu-ray profile (an AC-3 core with an E-AC-3 dependent substream).
- Verifies and re-signs EMDF protection with a key set you supply. Without keys, protected containers show as unverified and edits that would break them are refused.
