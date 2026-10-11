# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
