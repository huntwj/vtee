//! Golden files: the expected output of a screen test, and the comparison that checks a run against it.
//!
//! The format, the comparison and the reports are specified in `docs/design/snapshot-and-golden-contract.md`
//! (section 6); this module is its reference implementation.

pub mod escape;
mod model;
mod parse;
pub mod pattern;
mod version;

pub use model::{
    Assert, AssertText, Attr, AttrCheck, Entry, GRAMMAR_VERSION, Golden, GoldenScreen, Ignore, MODE_VOCABULARY, Mask,
    MaskText, Region, Semantics, attr_tokens, mode_key, shape_name, sorted_modes, underline_from_name, underline_name,
};
pub use parse::{LoadError, ParseError};
pub use version::{Build, Invocation, Refusal, WayForward};
