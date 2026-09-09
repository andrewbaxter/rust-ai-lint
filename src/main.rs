#![feature(rustc_private)]

mod collect;
mod driver;
mod style;
mod wire;

extern crate rustc_ast;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

use {
    crate::wire::{
        Comment,
        Def,
        OUT_DIR_ENV,
        Problem,
        Report,
        Use,
    },
    std::{
        collections::{
            BTreeMap,
            BTreeSet,
            HashMap,
        },
        ffi::OsStr,
        path::{
            Path,
            PathBuf,
        },
        process::{
            Command,
            exit,
        },
    },
};

fn cargo() -> PathBuf {
    return std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
}

/// Cargo skips crates whose sources haven't changed, and a skipped crate reports
/// nothing at all - which would read as "this crate has no code in it". Throwing
/// away what cargo built for the workspace's own crates makes every run see the
/// whole workspace; dependencies keep their cache, which is most of the time.
///
/// Cargo has moved this directory around between releases, so rather than knowing
/// the layout, look for anything named after a member crate.
fn clear_members(dir: &Path, members: &BTreeSet<String>, depth: usize) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let stem = name.rsplit_once('-').map(|(before, _)| before.to_string()).unwrap_or_else(|| name.clone());
        let named_after_member = [&name, &stem].into_iter().any(|candidate| {
            let underscored = candidate.replace('-', "_");
            return members.iter().any(|member| {
                return member == candidate || member.replace('-', "_") == underscored;
            });
        });
        if named_after_member {
            let _ = std::fs::remove_dir_all(&path);
            continue;
        }
        clear_members(&path, members, depth + 1);
    }
}

/// Every directory holding a manifest, skipping build output and anything hidden.
fn find_manifests(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    if dir.join("Cargo.toml").is_file() {
        found.push(dir.to_path_buf());
    }
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name.starts_with('.') {
            continue;
        }
        find_manifests(&path, found);
    }
}

/// Two roles share one binary. Cargo runs it as a compiler wrapper, with the real
/// rustc as the first argument and our output directory in the environment;
/// anything else is a person running the checker.
fn main() {
    let wrapping = std::env::var_os(OUT_DIR_ENV).is_some() && std::env::args_os().count() > 1;
    if wrapping {
        let mut args: Vec<String> = std::env::args().skip(1).collect();

        // The checker doesn't live in the toolchain, so rustc can't work out where the
        // standard library is from our own path; the real compiler cargo handed us knows.
        if !args.iter().any(|a| a == "--sysroot" || a.starts_with("--sysroot=")) {
            let found =
                Command::new(&args[0]).args(["--print", "sysroot"]).output().ok().filter(|o| o.status.success());
            if let Some(out) = found {
                args.push("--sysroot".to_string());
                args.push(String::from_utf8_lossy(&out.stdout).trim().to_string());
            }
        }
        rustc_driver::run_compiler(&args, &mut driver::Callbacks);
        return;
    }
    if std::env::args_os().count() > 1 {
        eprintln!(
            "rust-ai-lint takes no arguments: it checks every cargo project under the current directory, and every check it knows."
        );
        exit(2);
    }

    // Checks every cargo project under the current directory, then reports what it
    // found. Nothing here is optional and nothing is advisory: a non-empty report
    // means the tree is not acceptable.
    match (|| -> Result<i32, String> {
        let root = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {}", e))?;
        let mut manifests = vec![];
        find_manifests(&root, &mut manifests);
        if manifests.is_empty() {
            return Err(format!("no cargo project found under {}", root.display()));
        }

        // Ask cargo which workspace each manifest belongs to, so that a workspace with
        // many member crates is checked once rather than once per member.
        let mut workspaces: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
        for manifest in manifests {
            let out =
                Command::new(cargo())
                    .current_dir(&manifest)
                    .args(["metadata", "--no-deps", "--format-version=1"])
                    .output()
                    .map_err(|e| format!("failed to run cargo metadata in {}: {}", manifest.display(), e))?;
            if !out.status.success() {
                return Err(
                    format!(
                        "cargo metadata failed in {}: {}",
                        manifest.display(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    ),
                );
            }
            let meta: serde_json::Value =
                serde_json::from_slice(
                    &out.stdout,
                ).map_err(|e| format!("cargo metadata in {} is not json: {}", manifest.display(), e))?;
            let workspace_root =
                meta
                    .get("workspace_root")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| format!("cargo metadata in {} has no workspace_root", manifest.display()))?;
            let mut members = BTreeSet::new();
            for package in meta.get("packages").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                let Some(name) = package.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                members.insert(name.to_string());
            }
            workspaces.insert(PathBuf::from(workspace_root), members);
        }

        // Each crate the compiler wrapper sees writes what it found here.
        let out_dir = root.join("target").join("rust-ai-lint-reports");
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {}", out_dir.display(), e))?;
        let exe = std::env::current_exe().map_err(|e| format!("cannot find own path: {}", e))?;
        for (workspace_root, members) in &workspaces {
            let target = workspace_root.join("target").join("rust-ai-lint");
            clear_members(&target, members, 0);
            let status =
                Command::new(cargo())
                    .current_dir(workspace_root)
                    .args(["check", "--workspace", "--all-targets", "--quiet"])
                    .env("RUSTC_WORKSPACE_WRAPPER", &exe)
                    .env("CARGO_TARGET_DIR", &target)
                    .env(OUT_DIR_ENV, &out_dir)
                    .status()
                    .map_err(|e| format!("failed to run cargo check in {}: {}", workspace_root.display(), e))?;
            if !status.success() {
                return Err(
                    format!(
                        "{} does not compile, so it cannot be checked; fix the build errors above first",
                        workspace_root.display()
                    ),
                );
            }
        }
        let mut problems: Vec<Problem> = vec![];
        let mut defs: Vec<Def> = vec![];
        let mut uses: Vec<Use> = vec![];
        let mut comments: Vec<Comment> = vec![];
        let entries = std::fs::read_dir(&out_dir).map_err(|e| format!("cannot read {}: {}", out_dir.display(), e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() != Some(OsStr::new("json")) {
                continue;
            }
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let report: Report =
                serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {}", path.display(), e))?;
            problems.extend(report.problems);
            defs.extend(report.defs);
            uses.extend(report.uses);
            comments.extend(report.comments);
        }
        let _ = std::fs::remove_dir_all(&out_dir);
        if defs.is_empty() {
            return Err(
                format!(
                    "no crate reported anything, which means cargo reused old build output; remove {} and try again",
                    root.join("target").join("rust-ai-lint").display()
                ),
            );
        }

        // A def is dead if nothing names it, and asks to be dissolved if exactly one
        // hand-written name refers to it. Names a macro produced keep a def alive but
        // can't be rewritten, so they never ask for inlining.
        let mut sites: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut written: HashMap<String, BTreeSet<String>> = HashMap::new();
        for use_ in uses {
            sites.entry(use_.key.clone()).or_default().insert(use_.site.clone());
            if !use_.generated {
                written.entry(use_.key).or_default().insert(use_.site);
            }
        }
        let mut judged = BTreeSet::new();
        for def in defs {
            if def.exempt {
                continue;
            }
            if !judged.insert(def.key.clone()) {
                continue;
            }
            let total = sites.get(&def.key).map(|s| s.len()).unwrap_or(0);
            let by_hand = written.get(&def.key).map(|s| s.len()).unwrap_or(0);
            let message;
            if total == 0 {
                message = format!("{} `{}` is never used; delete it", def.kind, def.name);
            } else if def.inlinable && total == 1 && by_hand == 1 {
                message = format!("{} `{}` is used exactly once; inline it into its one caller", def.kind, def.name);
            } else {
                continue;
            }
            problems.push(Problem {
                check: "usage".to_string(),
                path: def.path,
                line: def.line,
                message: message,
            });
        }

        // Prose is written once. The same sentence in two places is a sign it was
        // generated rather than thought about, so every copy is reported.
        let mut by_text: BTreeMap<(String, String), Vec<Comment>> = BTreeMap::new();
        for comment in comments {
            if comment.kind == "directive" {
                continue;
            }
            by_text.entry((comment.kind.clone(), comment.text.clone())).or_default().push(comment);
        }
        for ((kind, text), mut group) in by_text {
            group.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
            group.dedup_by(|a, b| a.path == b.path && a.line == b.line);
            if group.len() < 2 {
                continue;
            }
            const LIMIT: usize = 70;
            let shown = if text.chars().count() <= LIMIT {
                text.clone()
            } else {
                format!("{}...", text.chars().take(LIMIT).collect::<String>())
            };
            for comment in group {
                problems.push(Problem {
                    check: "comments".to_string(),
                    path: comment.path,
                    line: comment.line,
                    message: format!(
                        "this {} appears verbatim elsewhere; say it once or not at all: {}",
                        kind,
                        shown
                    ),
                });
            }
        }
        for problem in &mut problems {
            problem.path =
                Path::new(&problem.path)
                    .strip_prefix(&root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| problem.path.clone());
        }
        problems.sort();
        problems.dedup();
        for problem in &problems {
            println!("{}:{}: [{}] {}", problem.path, problem.line, problem.check, problem.message);
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
            eprintln!("rust-ai-lint: {}", e);
            exit(2);
        },
    }
}
