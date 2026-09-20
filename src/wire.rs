use serde::{
    Deserialize,
    Serialize,
};

pub const OUT_DIR_ENV: &str = "RUST_AI_LINT_OUT";

#[derive(Serialize, Deserialize)]
pub struct Def {
    pub exempt: bool,
    pub inlinable: bool,
    pub key: String,
    pub kind: String,
    pub line: usize,
    pub name: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Problem {
    pub check: String,
    pub line: usize,
    pub message: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Report {
    pub defs: Vec<Def>,
    pub problems: Vec<Problem>,
    pub uses: Vec<Use>,
}

#[derive(Serialize, Deserialize)]
pub struct Use {
    pub generated: bool,
    pub key: String,
    pub site: String,
}
