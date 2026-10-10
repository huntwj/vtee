//! Which goldens this build will read (design sections 4.3, 5.3 and 6.8): the numbers must match exactly, and a
//! difference is a refusal with nothing compared. There is no window, no second reader and no override.

use super::model::{GRAMMAR_VERSION, Golden, Semantics};
use crate::snapshot::{SCHEMA_VERSION, SEMANTICS_VERSION};

/// The snapshot numbers of the running build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    pub schema: u32,
    pub semantics: u32,
}

impl Build {
    /// This build's own numbers.
    pub fn current() -> Build {
        Build { schema: SCHEMA_VERSION, semantics: SEMANTICS_VERSION }
    }
}

/// Why a golden cannot be compared. The run ends with exit code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The file's grammar number is above the one this build reads.
    Grammar { golden: u32, tool: u32 },
    /// The golden's `schema` differs from the build's.
    Schema { golden: u32, tool: u32 },
    /// The golden's `semantics` differs from the build's, or is `unknown`.
    Semantics { golden: Semantics, tool: u32 },
    /// A snapshot of the run carries another `schema_version` than the golden.
    SnapshotSchema { golden: u32, snapshot: u32 },
    /// A snapshot of the run carries another `semantics_version` than the golden.
    SnapshotSemantics { golden: u32, snapshot: u32 },
}

/// The command line of the run that is being reported, so a refusal can say what to type next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The golden's path as it was given.
    pub golden: String,
    /// The whole command, for example `vtee --script flow.ndjson --expect flow.golden -- tf -n`.
    pub command: String,
}

/// One suggested next step after a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayForward {
    /// The sentence, as printed under the `REFUSED` line (without its indent).
    pub text: String,
    /// The commands in it, for the JSON report.
    pub commands: Vec<String>,
}

impl Refusal {
    /// The text after `REFUSED NAME: `.
    pub fn reason(&self) -> String {
        match self {
            Refusal::Grammar { golden, tool } => {
                format!("grammar {golden} in the golden, {tool} in this vtee; nothing was compared")
            }
            Refusal::Schema { golden, tool } => {
                format!("schema {golden} in the golden, {tool} in this vtee; nothing was compared")
            }
            Refusal::Semantics { golden, tool } => {
                format!("semantics {golden} in the golden, {tool} in this vtee; nothing was compared")
            }
            Refusal::SnapshotSchema { golden, snapshot } => {
                format!("schema {golden} in the golden, {snapshot} in the snapshots; nothing was compared")
            }
            Refusal::SnapshotSemantics { golden, snapshot } => {
                format!("semantics {golden} in the golden, {snapshot} in the snapshots; nothing was compared")
            }
        }
    }

    /// What to do about it (section 5.3).
    pub fn way_forward(&self, run: &Invocation) -> Vec<WayForward> {
        let restamp = format!("{} --update=restamp", run.command);
        let record = format!("{} --update", run.command);
        let semantics = || {
            vec![
                WayForward {
                    text: format!("restamp (only if nothing the golden checks changed):  {restamp}"),
                    commands: vec![restamp.clone()],
                },
                WayForward {
                    text: "or re-record with --update; what changed is listed in docs/guide/snapshot-semantics.md"
                        .to_string(),
                    commands: vec![record.clone()],
                },
            ]
        };
        let migrate = || {
            let migrate = format!("vtee --expect {} --update=migrate", run.golden);
            vec![WayForward {
                text: format!("migrate the file:  {migrate}      (or re-record with --update)"),
                commands: vec![migrate, record.clone()],
            }]
        };
        let upgrade = |what: &str| vec![WayForward { text: format!("{what}; upgrade vtee"), commands: vec![] }];
        match self {
            Refusal::Grammar { .. } => upgrade("the golden was written by a newer vtee"),
            Refusal::Schema { golden, tool } if golden < tool => migrate(),
            Refusal::Schema { .. } => upgrade("the golden was written by a newer vtee"),
            Refusal::Semantics { .. } | Refusal::SnapshotSemantics { .. } => semantics(),
            Refusal::SnapshotSchema { golden, snapshot } if golden < snapshot => migrate(),
            Refusal::SnapshotSchema { .. } => upgrade("the snapshots were produced by an older vtee"),
        }
    }
}

impl Golden {
    /// Step 2 of the comparison (section 6.8): the golden's numbers against the build's, decided from the header
    /// alone. The grammar number was already checked when the file was read.
    pub fn check_build(&self, build: Build) -> Result<(), Refusal> {
        if self.grammar > GRAMMAR_VERSION {
            return Err(Refusal::Grammar { golden: self.grammar, tool: GRAMMAR_VERSION });
        }
        if self.schema != build.schema {
            return Err(Refusal::Schema { golden: self.schema, tool: build.schema });
        }
        match self.semantics {
            Semantics::Number(n) if n == build.semantics => Ok(()),
            other => Err(Refusal::Semantics { golden: other, tool: build.semantics }),
        }
    }

    /// Step 3: the numbers a snapshot carries against the golden's.
    pub fn check_snapshot(&self, schema: u32, semantics: u32) -> Result<(), Refusal> {
        if schema != self.schema {
            return Err(Refusal::SnapshotSchema { golden: self.schema, snapshot: schema });
        }
        match self.semantics {
            Semantics::Number(n) if n == semantics => Ok(()),
            Semantics::Number(n) => Err(Refusal::SnapshotSemantics { golden: n, snapshot: semantics }),
            Semantics::Unknown => Err(Refusal::Semantics { golden: Semantics::Unknown, tool: semantics }),
        }
    }
}
