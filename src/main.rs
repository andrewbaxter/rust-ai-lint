use {
    std::{
        collections::HashSet,
        path::PathBuf,
        process::exit,
    },
};

mod comments;
mod format;
mod git;
mod naming;
mod returns;
mod shorthand;
mod suppressions;
mod usage;

pub struct Source {
    pub path: String,
    pub text: String,
}

pub struct Problem {
    pub check: &'static str,
    pub path: String,
    pub line: usize,
    pub message: String,
}

const CHECKS: &[&str] = &["format", "comments", "naming", "returns", "shorthand", "suppressions", "usage"];

enum Mode {
    Staged,
    Worktree,
    Commit(String),
}

fn usage_text() -> String {
    return [
        "Usage: schemask-lint [options]",
        "",
        "Modes (default --staged):",
        "  --staged            check the git index against HEAD (for a pre-commit hook)",
        "  --worktree          check files on disk against HEAD",
        "  --commit <rev>      check an existing commit against its parent",
        "",
        "Options:",
        "  --root <dir>        repository to check (default: current directory)",
        "  --cargo <dir>       cargo workspace for the usage check, repeatable (default: all in repo)",
        "  --only <check>      run only these checks (repeatable); does not affect the hook",
        "  --rust-src <dir>    standard library sources (the dir holding core/ and std/)",
        "  -h, --help          this message",
        "",
        "Checks: format, comments, naming, returns, shorthand, suppressions, usage",
    ].join("\n");
}

fn collect_cargo_roots(dir: &std::path::Path, found: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let manifest = dir.join("Cargo.toml");
    if manifest.exists() &&
        std::fs::read_to_string(&manifest).map(|t| t.contains("[workspace]")).unwrap_or(false) {
        found.push(dir.to_path_buf());
    }
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" || name.starts_with('.') {
            continue;
        }
        collect_cargo_roots(&path, found);
    }
}

fn main() {
    match (|| -> Result<i32, String> {
        let mut mode = Mode::Staged;
        let mut root = None;
        let mut cargo: Vec<PathBuf> = vec![];
        let mut only: HashSet<String> = HashSet::new();
        let mut rust_src = None;
        let mut argv = std::env::args().skip(1);
        while let Some(arg) = argv.next() {
            let mut value = || -> Result<String, String> {
                return argv.next().ok_or_else(|| format!("{} needs a value", arg));
            };
            match arg.as_str() {
                "-h" | "--help" => {
                    println!("{}", usage_text());
                    exit(0);
                },
                "--staged" => mode = Mode::Staged,
                "--worktree" => mode = Mode::Worktree,
                "--commit" => mode = Mode::Commit(value()?),
                "--root" => root = Some(PathBuf::from(value()?)),
                "--cargo" => cargo.push(PathBuf::from(value()?)),
                "--only" => {
                    only.insert(value()?);
                },
                "--rust-src" => rust_src = Some(PathBuf::from(value()?)),
                other => return Err(format!("unrecognized argument `{}`\n\n{}", other, usage_text())),
            }
        }
        for name in only.iter() {
            if !CHECKS.contains(&name.as_str()) {
                return Err(format!("unknown check `{}`; known checks: {}", name, CHECKS.join(", ")));
            }
        }
        let enabled: HashSet<String> =
            CHECKS.iter().map(|c| c.to_string()).filter(|c| only.is_empty() || only.contains(c)).collect();
        let root = git::root(&root.unwrap_or_else(|| PathBuf::from(".")))?;
        let (old, new) = match &mode {
            Mode::Staged => {
                let old = if git::rev_exists(&root, "HEAD") {
                    git::tree_sources(&root, "HEAD")?
                } else {
                    vec![]
                };
                (old, git::index_sources(&root)?)
            },
            Mode::Worktree => {
                let old = if git::rev_exists(&root, "HEAD") {
                    git::tree_sources(&root, "HEAD")?
                } else {
                    vec![]
                };
                (old, git::worktree_sources(&root)?)
            },
            Mode::Commit(rev) => {
                let parent = format!("{}^", rev);
                let old = if git::rev_exists(&root, &parent) {
                    git::tree_sources(&root, &parent)?
                } else {
                    vec![]
                };
                (old, git::tree_sources(&root, rev)?)
            },
        };
        let mut problems = vec![];
        if enabled.contains("format") {
            let config = format::load_config(&root)?;
            format::check(&new, &config, &mut problems);
        }
        if enabled.contains("comments") {
            if !old.is_empty() {
                comments::check(&old, &new, &mut problems);
            }
        }
        if enabled.contains("naming") {
            naming::check(&new, &mut problems);
        }
        if enabled.contains("returns") {
            returns::check(&new, &mut problems);
        }
        if enabled.contains("shorthand") {
            shorthand::check(&new, &mut problems);
        }
        if enabled.contains("suppressions") {
            suppressions::check(&new, &mut problems);
        }
        if enabled.contains("usage") {
            let roots = if cargo.is_empty() {
                let mut found = vec![];
                collect_cargo_roots(&root, &mut found);
                if found.is_empty() && root.join("Cargo.toml").exists() {
                    found.push(root.clone());
                }
                found.sort();
                found
            } else {
                cargo
            };
            if roots.is_empty() {
                return Err("no cargo workspace found for the usage check".to_string());
            }
            for cargo_root in roots {
                usage::check(&cargo_root, &usage::Config {
                    rust_src: rust_src.clone(),
                    report_root: root.clone(),
                    derives_expand: true,
                }, &mut problems)?;
            }
        }
        problems.sort_by(|a, b| (a.path.as_str(), a.line, a.check).cmp(&(b.path.as_str(), b.line, b.check)));
        for problem in &problems {
            if problem.path.is_empty() {
                println!("[{}] {}", problem.check, problem.message);
            } else {
                println!("{}:{}: [{}] {}", problem.path, problem.line, problem.check, problem.message);
            }
        }
        if problems.is_empty() {
            return Ok(0);
        }
        println!("");
        println!("{} problem(s)", problems.len());
        return Ok(1);
    })() {
        Ok(code) => exit(code),
        Err(e) => {
            eprintln!("schemask-lint: {}", e);
            exit(2);
        },
    }
}
