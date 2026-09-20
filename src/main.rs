#![feature(rustc_private)]

mod collect;
mod comments;
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
        Def,
        OUT_DIR_ENV,
        Problem,
        Report,
        Use,
    },
    genemichaels_lib::{
        CommentMode,
        FormatConfig,
        WhitespaceMode,
        extract_whitespaces,
        format_ast,
    },
    std::{
        collections::{
            BTreeMap,
            BTreeSet,
            HashMap,
        },
        ffi::OsStr,
        io::Write,
        path::{
            Path,
            PathBuf,
        },
        process::{
            Command,
            Stdio,
            exit,
        },
    },
};

const USAGE: &str =
    "Usage: rust-ai-lint [--against <rev>]\n\nChecks every cargo project under the current directory, and restores the\ncomments of everything staged for commit to what they were at <rev>\n(default HEAD). Every check is run; none of them can be turned off.";

fn cargo() -> PathBuf {
    return std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"));
}

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

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out =
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run git {:?}: {}", args, e))?;
    if !out.status.success() {
        return Err(format!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr).trim()));
    }
    return Ok(String::from_utf8_lossy(&out.stdout).to_string());
}

fn main() {
    let wrapping = std::env::var_os(OUT_DIR_ENV).is_some() && std::env::args_os().count() > 1;
    if wrapping {
        let mut args: Vec<String> = std::env::args().skip(1).collect();
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
    match (|| -> Result<i32, String> {
        let mut against = "HEAD".to_string();
        let mut argv = std::env::args().skip(1);
        while let Some(arg) = argv.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    println!("{}", USAGE);
                    exit(0);
                },
                "--against" => {
                    against = argv.next().ok_or_else(|| format!("--against needs a revision\n\n{}", USAGE))?;
                },
                other => return Err(format!("unrecognized argument `{}`\n\n{}", other, USAGE)),
            }
        }
        let root = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {}", e))?;
        let mut problems: Vec<Problem> = vec![];
        let repo = PathBuf::from(git(&root, &["rev-parse", "--show-toplevel"])?.trim());
        let known = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", against)]).is_ok();
        if !known && against != "HEAD" {
            return Err(format!("no commit named `{}`", against));
        }
        let mut was: BTreeMap<String, String> = BTreeMap::new();
        if known {
            for path in git(&repo, &["ls-tree", "-r", "--name-only", "-z", &against])?.split('\0') {
                if !path.ends_with(".rs") {
                    continue;
                }
                was.insert(path.to_string(), git(&repo, &["show", &format!("{}:{}", against, path)])?);
            }
        }
        let mut unstaged = vec![];
        for entry in git(&repo, &["ls-files", "--stage", "-z"])?.split('\0') {
            let Some((meta, path)) = entry.split_once('\t') else {
                continue;
            };
            if !path.ends_with(".rs") {
                continue;
            }
            let Some(mode) = meta.split_whitespace().next() else {
                continue;
            };
            let staged = git(&repo, &["show", &format!(":{}", path)])?;
            let at = repo.join(path);
            let config = (|| {
                let mut dir = at.parent();
                while let Some(here) = dir {
                    for name in [".genemichaels.json", "genemichaels.json"] {
                        let Ok(found) = std::fs::read_to_string(here.join(name)) else {
                            continue;
                        };
                        let Ok(parsed) = serde_json::from_str(&found) else {
                            continue;
                        };
                        return parsed;
                    }
                    dir = here.parent();
                }
                return FormatConfig::default();
            })();
            let fixed = (|| -> Result<Option<String>, String> {
                let (shebang, body) = match staged.starts_with("#!/") {
                    false => (None, staged.as_str()),
                    true => {
                        let end = staged.find('\n').map(|o| o + 1).unwrap_or(staged.len());
                        (Some(&staged[..end]), &staged[end..])
                    },
                };
                let offset = shebang.map(|_| 1).unwrap_or(0);
                let (mut whitespaces, tokens) =
                    extract_whitespaces(
                        config.keep_max_blank_lines,
                        body,
                    ).map_err(|e| format!("cannot be read: {}", e))?;
                for group in whitespaces.values() {
                    for whitespace in group {
                        let WhitespaceMode::Comment(comment) = &whitespace.mode else {
                            continue;
                        };
                        if comment.mode == CommentMode::Directive &&
                            comment.lines.lines().any(|line| line.trim() == "genemichaels-file-skip") {
                            return Ok(None);
                        }
                    }
                }
                let spots = comments::walk(&whitespaces, body);
                let before = match was.get(path) {
                    None => vec![],
                    Some(old) => {
                        let (whitespaces, _) =
                            extract_whitespaces(
                                config.keep_max_blank_lines,
                                old,
                            ).map_err(|e| format!("cannot be read: {}", e))?;
                        comments::walk(&whitespaces, old)
                    },
                };
                let anchored = comments::pairs(&before, &spots, |old, new| return old.code == new.code, |old, new| {
                    let left = old.code.split_whitespace().collect::<BTreeSet<_>>();
                    let right = new.code.split_whitespace().collect::<BTreeSet<_>>();
                    let most = left.len().max(right.len());
                    return most > 0 && left.intersection(&right).count() * 2 >= most;
                });
                let mut restore = vec![];
                let mut remove = vec![];
                for (into, spot) in spots.iter().enumerate() {
                    let older: &[genemichaels_lib::Comment] = match anchored[into] {
                        Some(from) => &before[from].comments,
                        None => &[],
                    };
                    let matched =
                        comments::pairs(
                            older,
                            &spot.comments,
                            |old, new| return old.mode == new.mode &&
                                comments::collapse(&old.lines) == comments::collapse(&new.lines),
                            |old, new| return old.mode == new.mode,
                        );
                    for (index, comment) in spot.comments.iter().enumerate() {
                        let Some(from) = matched[index] else {
                            println!(
                                "{}:{}: [comments] removed a comment that was not there before: {}",
                                path,
                                spot.lines[index] + offset,
                                comments::shown(comment)
                            );
                            remove.push((into, index));
                            continue;
                        };
                        if comments::collapse(&older[from].lines) == comments::collapse(&comment.lines) {
                            continue;
                        }
                        println!(
                            "{}:{}: [comments] put back a comment that had been reworded: {}",
                            path,
                            spot.lines[index] + offset,
                            comments::shown(comment)
                        );
                        restore.push((into, index, older[from].clone()));
                    }
                }
                for (into, index, source) in restore {
                    let spot = &spots[into].at[index];
                    let group =
                        whitespaces.get_mut(&spot.key).ok_or_else(|| "a comment left its anchor".to_string())?;
                    let WhitespaceMode::Comment(comment) = &mut group[spot.index].mode else {
                        return Err("a comment left its anchor".to_string());
                    };
                    comment.lines = source.lines;
                    comment.mode = source.mode;
                }
                remove.sort_by_key(|(into, index)| {
                    let spot = &spots[*into].at[*index];
                    return std::cmp::Reverse((spot.key, spot.index));
                });
                for (into, index) in remove {
                    let spot = &spots[into].at[index];
                    let group =
                        whitespaces.get_mut(&spot.key).ok_or_else(|| "a comment left its anchor".to_string())?;
                    group.remove(spot.index);
                }
                let parsed = syn::parse2::<syn::File>(tokens).map_err(|e| format!("cannot be parsed: {}", e))?;
                let done =
                    format_ast(parsed, &config, whitespaces).map_err(|e| format!("cannot be formatted: {}", e))?;
                if !done.lost_comments.is_empty() {
                    return Err(
                        format!(
                            "formatting would drop {} comment(s); move them somewhere the formatter can keep",
                            done.lost_comments.len()
                        ),
                    );
                }
                return Ok(Some(match shebang {
                    Some(shebang) => format!("{}{}", shebang, done.rendered),
                    None => done.rendered,
                }));
            })();
            let rendered = match fixed {
                Ok(None) => continue,
                Ok(Some(rendered)) => rendered,
                Err(e) => {
                    problems.push(Problem {
                        check: "comments".to_string(),
                        path: path.to_string(),
                        line: 1,
                        message: e,
                    });
                    continue;
                },
            };
            if rendered == staged {
                continue;
            }
            let mut child =
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(["hash-object", "-w", "--stdin"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("failed to run git hash-object: {}", e))?;
            child
                .stdin
                .take()
                .ok_or_else(|| "git hash-object took no stdin".to_string())?
                .write_all(rendered.as_bytes())
                .map_err(|e| format!("failed to hand {} to git hash-object: {}", path, e))?;
            let out = child.wait_with_output().map_err(|e| format!("failed to run git hash-object: {}", e))?;
            if !out.status.success() {
                return Err("git hash-object failed".to_string());
            }
            let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
            git(&repo, &["update-index", "--cacheinfo", &format!("{},{},{}", mode, hash, path)])?;
            match std::fs::read_to_string(&at) {
                Ok(text) if text == staged => {
                    std::fs::write(&at, &rendered).map_err(|e| format!("cannot write {}: {}", at.display(), e))?;
                },
                Ok(_) => unstaged.push(path.to_string()),
                Err(_) => { },
            }
        }
        for path in &unstaged {
            println!(
                "{}:1: [comments] fixed in the commit only; the copy on disk has unstaged edits and was left alone",
                path
            );
        }
        let mut manifests = vec![];
        find_manifests(&root, &mut manifests);
        if manifests.is_empty() {
            return Err(format!("no cargo project found under {}", root.display()));
        }
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
        let mut defs: Vec<Def> = vec![];
        let mut uses: Vec<Use> = vec![];
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
            println!("rust-ai-lint: {}", e);
            exit(1);
        },
    }
}
