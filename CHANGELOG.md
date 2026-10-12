# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/huntwj/vtee/compare/v0.1.0...v0.1.1) - 2026-10-12

### Fixed

- Tap::spawn enforces the same terminal size bound as resize
- script-driver error handling for two cases Command::execute doesn't own
- Windows console-close now cleans up session files; log a failed own-SID lookup
- checking attach_if_exists no longer creates a config file
- --no-listen no longer leaves a diagnostic log file behind
- reset the pen around attach's redraw; document two unfixable library gaps
- don't hot-spin a core when accept() fails persistently
- a broken stdout pipe no longer freezes the whole session
- don't panic on a non-UTF8 program argument before clap runs
- attach exits by itself once the session it's watching ends
- reassemble UTF-8 characters split across --log chunk boundaries
- don't delete or steal someone else's file at a --listen path
- write-lock re-acquire, attach's exclusive-shared revoke, and SIGHUP cleanup
- normalize CRLF to LF in files that should follow the repo's eol=lf policy

### Performance

- wait_for only builds a full snapshot once it's about to return

## [0.1.0](https://github.com/huntwj/vtee/compare/v0.0.1...v0.1.0) - 2026-10-11

### Added

- *(cli)* --update records a golden, keeping what was written by hand; add --update=restamp
- *(cli)* --expect checks a run against a golden
- *(cli)* --connect with --script runs the script against the live session
- *(script)* a timed-out wait_for fails the run unless it is optional
- *(golden)* re-record a golden and keep what was written by hand
- *(golden)* read saved output against a build instead of a golden
- *(golden)* check a run against a golden and report the verdict as text and JSON
- *(golden)* compare a run with a golden: pairing, masks, assertions, fail-fast
- *(golden)* write goldens in their canonical form and record them from snapshots
- *(golden)* read goldens and decide which versions are readable
- *(golden)* the regex dialect for masks and assertions
- *(golden)* the escape set and the cell and string encodings

### Changed

- *(golden)* keep the program last in a refusal's suggested command
