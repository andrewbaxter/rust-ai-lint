use serde::{
    Deserialize,
    Serialize,
};

pub const OUT_DIR_ENV: &str = "RUST_AI_LINT_OUT";

/// One comment, for finding text repeated across the whole tree.
#[derive(Serialize, Deserialize)]
pub struct Comment {
    pub kind: String,
    pub line: usize,
    pub path: String,
    pub text: String,
}

/// Something that can be used: an item, a field, a variant, or a binding. Keys are
/// stable across separately compiled crates and across recompilations of the same
/// crate, so the orchestrator can add up uses that are spread over a workspace.
#[derive(Serialize, Deserialize)]
pub struct Def {
    /// Set when the definition can't be judged from inside the crate: it is reachable
    /// from outside, or the compiler/test harness calls it for us.
    pub exempt: bool,
    /// Set when a single use could be dissolved into its caller, which is true of
    /// functions, consts and statics but not of types, fields or variants.
    pub inlinable: bool,
    pub key: String,
    pub kind: String,
    pub line: usize,
    pub name: String,
    pub path: String,
}

/// A rule violation. Every problem is fatal; there are no warnings.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Problem {
    pub check: String,
    pub line: usize,
    pub message: String,
    pub path: String,
}

/// What one compilation of one crate found.
#[derive(Serialize, Deserialize, Default)]
pub struct Report {
    pub comments: Vec<Comment>,
    pub defs: Vec<Def>,
    pub problems: Vec<Problem>,
    pub uses: Vec<Use>,
}

/// One place a def is named. Identified by source location so that compiling the
/// same file twice (a lib and its test harness, say) counts as one use.
#[derive(Serialize, Deserialize)]
pub struct Use {
    /// Set when the name was written by a macro rather than by a person. Such a use
    /// keeps the def alive but can't be rewritten, so it never asks for inlining.
    pub generated: bool,
    pub key: String,
    pub site: String,
}
