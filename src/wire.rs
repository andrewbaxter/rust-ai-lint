use serde::{
    Deserialize,
    Serialize,
};

pub const WRAP_ENV: &str = "RUST_AI_LINT_WRAP";

pub fn report_name(extra_filename: &str) -> String {
    return format!("rust-ai-lint{}.json", extra_filename);
}

#[derive(Serialize, Deserialize)]
pub struct Def {
    pub exempt: bool,
    pub inlinable: bool,
    pub key: String,
    pub kind: String,
    pub line: usize,
    pub movable: bool,
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
    pub functions: Vec<String>,
    pub generated: bool,
    pub key: String,
    pub site: String,
    pub test: bool,
}
