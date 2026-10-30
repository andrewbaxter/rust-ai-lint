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
    aargvark::{
        Aargvark,
        traits_impls::NotFlag,
        vark,
    },
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

#[derive(Aargvark)]
struct Args {
    against: Option<String>,
    features: Option<Vec<NotFlag>>,
    all_features: Option<()>,
    no_default_features: Option<()>,
}

const WASM_TARGET: &str = "wasm32-unknown-unknown";

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

fn report(mut problems: Vec<Problem>, root: &Path) -> i32 {
    for problem in &mut problems {
        problem.path =
            Path::new(&problem.path)
                .strip_prefix(root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| problem.path.clone());
    }
    problems.sort();
    problems.dedup();
    for problem in &problems {
        println!("{}:{}: [{}] {}", problem.path, problem.line, problem.check, problem.message);
    }
    if problems.is_empty() {
        return 0;
    }
    println!("");
    println!("{} problem(s)", problems.len());
    return 1;
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

fn apply(
    repo: &Path,
    path: &str,
    mode: &str,
    at: &Path,
    source: &str,
    rendered: &str,
    committing: bool,
) -> Result<(), String> {
    if rendered == source {
        return Ok(());
    }
    if !committing {
        std::fs::write(at, rendered).map_err(|e| format!("cannot write {}: {}", at.display(), e))?;
        return Ok(());
    }
    let mut child =
        Command::new("git")
            .arg("-C")
            .arg(repo)
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
    git(repo, &["update-index", "--cacheinfo", &format!("{},{},{}", mode, hash, path)])?;
    if std::fs::read_to_string(at).is_ok_and(|text| text == source) {
        std::fs::write(at, rendered).map_err(|e| format!("cannot write {}: {}", at.display(), e))?;
    }
    return Ok(());
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
        let args = vark::<Args>();
        let against = args.against.unwrap_or_else(|| "HEAD".to_string());
        let features: Vec<String> = args.features.unwrap_or_default().into_iter().map(|f| f.0).collect();
        let root = std::env::current_dir().map_err(|e| format!("cannot read the current directory: {}", e))?;
        let mut problems: Vec<Problem> = vec![];
        let repo = PathBuf::from(git(&root, &["rev-parse", "--show-toplevel"])?.trim());
        let known = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", against)]).is_ok();
        if !known && against != "HEAD" {
            return Err(format!("no commit named `{}`", against));
        }
        let committing = std::env::var_os("GIT_INDEX_FILE").is_some();
        let relevant = |path: &str| {
            return path.ends_with(".rs") || path.ends_with(".md") ||
                path.rsplit('/').next() == Some("Cargo.toml");
        };
        let mut changed: BTreeSet<String> = BTreeSet::new();
        let mut was: BTreeMap<String, String> = BTreeMap::new();
        if known {
            let mut diff = vec!["diff", "--name-status", "--no-renames", "-z"];
            if committing {
                diff.push("--cached");
            }
            diff.push(&against);
            let listed = git(&repo, &diff)?;
            let mut fields = listed.split('\0');
            while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
                if !relevant(path) {
                    continue;
                }
                changed.insert(path.to_string());
                if status != "A" && !path.ends_with("Cargo.toml") {
                    was.insert(path.to_string(), git(&repo, &["show", &format!("{}:{}", against, path)])?);
                }
            }
        } else {
            changed.extend(
                git(&repo, &["ls-files", "-z"])?.split('\0').filter(|path| relevant(path)).map(String::from),
            );
        }
        if !committing {
            changed.extend(
                git(&repo, &["ls-files", "--others", "--exclude-standard", "-z"])?
                    .split('\0')
                    .filter(|path| relevant(path))
                    .map(String::from),
            );
        }
        for entry in git(&repo, &["ls-files", "--stage", "-z"])?.split('\0') {
            let Some((meta, path)) = entry.split_once('\t') else {
                continue;
            };
            let markdown = path.ends_with(".md");
            if !path.ends_with(".rs") && !markdown || !changed.contains(path) {
                continue;
            }
            let Some(mode) = meta.split_whitespace().next() else {
                continue;
            };
            let at = repo.join(path);
            let source = match committing {
                true => git(&repo, &["show", &format!(":{}", path)])?,
                false => match std::fs::read_to_string(&at) {
                    Ok(text) => text,
                    Err(_) => continue,
                },
            };
            if markdown {
                let Some(older) = was.get(path) else {
                    continue;
                };
                apply(&repo, path, mode, &at, &source, older, committing)?;
                continue;
            }
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
                let (shebang, body) = match source.starts_with("#!/") {
                    false => (None, source.as_str()),
                    true => {
                        let end = source.find('\n').map(|o| o + 1).unwrap_or(source.len());
                        (Some(&source[..end]), &source[end..])
                    },
                };
                let (mut whitespaces, mut block_whitespaces, tokens) =
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
                let spots = comments::walk(&whitespaces, &block_whitespaces, body);
                let before = match was.get(path) {
                    None => vec![],
                    Some(old) => {
                        let (whitespaces, block_whitespaces, _) =
                            extract_whitespaces(
                                config.keep_max_blank_lines,
                                old,
                            ).map_err(|e| format!("cannot be read: {}", e))?;
                        comments::walk(&whitespaces, &block_whitespaces, old)
                    },
                };
                let anchored = comments::pairs(&before, &spots, |old, new| return old.code == new.code, |old, new| {
                    let left = old.code.split_whitespace().collect::<BTreeSet<_>>();
                    let right = new.code.split_whitespace().collect::<BTreeSet<_>>();
                    let most = left.len().max(right.len());
                    return most > 0 && left.intersection(&right).count() * 2 >= most;
                });
                let mut restore = vec![];
                let mut remove = BTreeSet::new();
                let mut inserts = vec![];
                let mut used = BTreeSet::new();
                for (into, spot) in spots.iter().enumerate() {
                    let older: &[genemichaels_lib::Comment] = match anchored[into] {
                        Some(from) => {
                            used.insert(from);
                            &before[from].comments
                        },
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
                    let mut kept = vec![
                        None;
                        older.len()
                    ];
                    for (index, comment) in spot.comments.iter().enumerate() {
                        let Some(from) = matched[index] else {
                            remove.insert((spot.at[index].inner, spot.at[index].key, spot.at[index].index));
                            continue;
                        };
                        kept[from] = Some(index);
                        if comments::collapse(&older[from].lines) == comments::collapse(&comment.lines) {
                            continue;
                        }
                        restore.push((into, index, older[from].clone()));
                    }
                    for (from, comment) in older.iter().enumerate() {
                        if kept[from].is_some() {
                            continue;
                        }
                        let pos = match kept[..from].iter().rev().flatten().next() {
                            Some(index) => spot.at[*index].index + 1,
                            None => spot.at[kept[from..].iter().flatten().next().copied().unwrap_or(0)].index,
                        };
                        inserts.push(comments::Insert {
                            inner: spot.at[0].inner,
                            key: spot.at[0].key,
                            pos: pos,
                            comment: comment.clone(),
                            relocated: false,
                        });
                    }
                }
                if let Some(old) = was.get(path) {
                    let taken =
                        spots
                            .iter()
                            .enumerate()
                            .filter(|(into, _)| anchored[*into].is_some())
                            .map(|(_, spot)| (spot.at[0].inner, spot.at[0].key))
                            .collect::<BTreeSet<_>>();
                    for (from, spot) in before.iter().enumerate() {
                        if used.contains(&from) {
                            continue;
                        }
                        let inner = spot.at[0].inner;
                        let Some(key) = (|| -> Option<genemichaels_lib::HashLineColumn> {
                            if spot.at[0].key.0.line == 0 {
                                return Some(spot.at[0].key);
                            }
                            let old_lines = old.lines().collect::<Vec<_>>();
                            let new_lines = body.lines().collect::<Vec<_>>();
                            let line = [false, true].into_iter().find_map(|context| {
                                let label = |lines: &[&str], line: usize| {
                                    let mut out = match inner {
                                        true => comments::collapse(lines[line - 1]),
                                        false => comments::anchor(lines, line),
                                    };
                                    if context {
                                        let before =
                                            lines[..line - 1].iter().map(|text| text.trim()).rev().find(|text| {
                                                return !text.is_empty() && !text.starts_with("//");
                                            });
                                        out = format!("{}\n{}", before.unwrap_or(""), out);
                                    }
                                    return out;
                                };
                                let code = label(&old_lines, spot.at[0].key.0.line);
                                let mut found =
                                    (1 ..= new_lines.len()).filter(|line| label(&new_lines, *line) == code);
                                let line = found.next()?;
                                if found.next().is_some() ||
                                    (1 ..= old_lines.len())
                                        .filter(|line| label(&old_lines, *line) == code)
                                        .count() !=
                                        1 {
                                    return None;
                                }
                                return Some(line);
                            })?;
                            let was = old_lines[spot.at[0].key.0.line - 1];
                            let now = new_lines[line - 1];
                            if was.trim() != now.trim() {
                                return None;
                            }
                            let indent = |text: &str| text.chars().count() - text.trim_start().chars().count();
                            let mut moved = spot.at[0].key;
                            moved.0.line = line;
                            moved.0.column = (spot.at[0].key.0.column + indent(now)).checked_sub(indent(was))?;
                            return Some(moved);
                        })() else {
                            continue;
                        };
                        if taken.contains(&(inner, key)) {
                            continue;
                        }
                        for comment in &spot.comments {
                            inserts.push(comments::Insert {
                                inner: inner,
                                key: key,
                                pos: usize::MAX,
                                comment: comment.clone(),
                                relocated: true,
                            });
                        }
                    }
                }
                for (into, index, older) in restore {
                    let spot = &spots[into].at[index];
                    let group = match spot.inner {
                        true => &mut block_whitespaces,
                        false => &mut whitespaces,
                    }.get_mut(&spot.key).ok_or_else(|| "a comment left its anchor".to_string())?;
                    let WhitespaceMode::Comment(comment) = &mut group[spot.index].mode else {
                        return Err("a comment left its anchor".to_string());
                    };
                    comment.lines = older.lines;
                    comment.mode = older.mode;
                }
                let parsed = syn::parse2::<syn::File>(tokens).map_err(|e| format!("cannot be parsed: {}", e))?;
                let mut old_docs = BTreeMap::<String, usize>::new();
                if let Some(old) = was.get(path) {
                    let old_parsed =
                        extract_whitespaces(config.keep_max_blank_lines, old)
                            .ok()
                            .and_then(|(_, _, tokens)| syn::parse2::<syn::File>(tokens).ok());
                    for (text, _) in old_parsed.iter().flat_map(comments::doc_attributes) {
                        *old_docs.entry(text).or_default() += 1;
                    }
                }
                for (text, line) in comments::doc_attributes(&parsed) {
                    match old_docs.get_mut(&text) {
                        Some(count) if *count > 0 => {
                            *count -= 1;
                        },
                        _ => problems.push(Problem {
                            check: "comments".to_string(),
                            path: path.to_string(),
                            line: line,
                            message: "doc attributes can't be added or changed, the same as comments; remove it".to_string(),
                        }),
                    }
                }
                let format = |relocated: bool| {
                    let mut whitespaces = whitespaces.clone();
                    let mut block_whitespaces = block_whitespaces.clone();
                    for inner in [false, true] {
                        let chosen =
                            inserts
                                .iter()
                                .filter(|insert| insert.inner == inner && (relocated || !insert.relocated))
                                .collect::<Vec<_>>();
                        let removed =
                            remove
                                .iter()
                                .filter(|(at_inner, _, _)| *at_inner == inner)
                                .map(|(_, key, index)| (*key, *index))
                                .collect::<BTreeSet<_>>();
                        let map = match inner {
                            true => &mut block_whitespaces,
                            false => &mut whitespaces,
                        };
                        let keys =
                            removed
                                .iter()
                                .map(|(key, _)| *key)
                                .chain(chosen.iter().map(|insert| insert.key))
                                .collect::<BTreeSet<_>>();
                        for key in keys {
                            let old = map.remove(&key).unwrap_or_default();
                            let mut new = vec![];
                            for pos in 0 ..= old.len() {
                                for insert in chosen
                                    .iter()
                                    .filter(|insert| insert.key == key && insert.pos.min(old.len()) == pos) {
                                    new.push(genemichaels_lib::Whitespace {
                                        loc: key.0,
                                        mode: WhitespaceMode::Comment(insert.comment.clone()),
                                    });
                                }
                                if pos < old.len() && !removed.contains(&(key, pos)) {
                                    new.push(old[pos].clone());
                                }
                            }
                            if !new.is_empty() {
                                map.insert(key, new);
                            }
                        }
                    }
                    return format_ast(
                        parsed.clone(),
                        &config,
                        whitespaces,
                        block_whitespaces,
                    ).map_err(|e| format!("cannot be formatted: {}", e));
                };
                let mut done = format(true)?;
                if !done.lost_comments.is_empty() && inserts.iter().any(|insert| insert.relocated) {
                    done = format(false)?;
                }
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
            apply(&repo, path, mode, &at, &source, &rendered, committing)?;
        }
        let real_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let mut manifests = BTreeSet::new();
        for path in changed.iter().filter(|path| !path.ends_with(".md")) {
            let mut dir = repo.join(path).parent().map(Path::to_path_buf);
            while let Some(here) = dir {
                if !here.starts_with(&repo) {
                    break;
                }
                if here.join("Cargo.toml").is_file() {
                    if here.starts_with(&real_root) {
                        manifests.insert(here);
                    }
                    break;
                }
                dir = here.parent().map(Path::to_path_buf);
            }
        }
        if manifests.is_empty() {
            return Ok(report(problems, &root));
        }
        let mut workspaces: BTreeMap<PathBuf, BTreeMap<String, BTreeSet<Option<String>>>> = BTreeMap::new();
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
            let mut members = BTreeMap::new();
            for package in meta.get("packages").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                let Some(name) = package.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let mut targets = BTreeSet::new();
                let mut native = true;
                for dep in package.get("dependencies").and_then(|v| v.as_array()).unwrap_or(&vec![]) {
                    if dep.get("kind").is_some_and(|k| !k.is_null()) {
                        continue;
                    }
                    match dep.get("target").and_then(|v| v.as_str()) {
                        None => {
                            let dep_name = dep.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            if ["wasm-bindgen", "js-sys", "web-sys"].contains(&dep_name) {
                                native = false;
                                targets.insert(Some(WASM_TARGET.to_string()));
                            }
                        },
                        Some(platform) => {
                            if platform.contains("wasm32") {
                                targets.insert(Some(WASM_TARGET.to_string()));
                            }
                        },
                    }
                }
                if native {
                    targets.insert(None);
                }
                members.insert(name.to_string(), targets);
            }
            workspaces.insert(PathBuf::from(workspace_root), members);
        }
        let out_dir = root.join("target").join("rust-ai-lint-reports");
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {}", out_dir.display(), e))?;
        let exe = std::env::current_exe().map_err(|e| format!("cannot find own path: {}", e))?;
        for (workspace_root, members) in &workspaces {
            let target = workspace_root.join("target").join("rust-ai-lint");
            clear_members(&target, &members.keys().cloned().collect(), 0);
            let mut groups: BTreeMap<Option<String>, BTreeSet<String>> = BTreeMap::new();
            for (member, member_targets) in members {
                for member_target in member_targets {
                    groups.entry(member_target.clone()).or_default().insert(member.clone());
                }
            }
            for (group_target, packages) in &groups {
                let mut command = Command::new(cargo());
                command.current_dir(workspace_root).args(["check", "--all-targets", "--quiet"]);
                for package in packages {
                    command.args(["-p", package]);
                }
                if let Some(group_target) = group_target {
                    command.args(["--target", group_target]);
                }
                for feature in &features {
                    let other_member = feature.split_once('/').is_some_and(|(package, _)| {
                        return members.contains_key(package) && !packages.contains(package);
                    });
                    if !other_member {
                        command.args(["--features", feature]);
                    }
                }
                if args.all_features.is_some() {
                    command.arg("--all-features");
                }
                if args.no_default_features.is_some() {
                    command.arg("--no-default-features");
                }
                let status =
                    command
                        .env("RUSTC_BOOTSTRAP", "1")
                        .env("RUSTC_WORKSPACE_WRAPPER", &exe)
                        .env("CARGO_TARGET_DIR", &target)
                        .env(OUT_DIR_ENV, &out_dir)
                        .status()
                        .map_err(|e| format!("failed to run cargo check in {}: {}", workspace_root.display(), e))?;
                if !status.success() {
                    return Err(
                        format!(
                            "{} does not compile for {}, so it cannot be checked; fix the build errors above first",
                            workspace_root.display(),
                            group_target.as_deref().unwrap_or("the default target")
                        ),
                    );
                }
            }
        }
        let mut defs: Vec<Def> = vec![];
        let mut uses: Vec<Use> = vec![];
        let real_repo = repo.canonicalize().unwrap_or_else(|_| repo.clone());
        let build_dirs: Vec<PathBuf> = workspaces.keys().map(|workspace_root| {
            let target = workspace_root.join("target").join("rust-ai-lint");
            return target.canonicalize().unwrap_or(target);
        }).collect();
        let mut seen: HashMap<String, bool> = HashMap::new();
        let mut ours = |path: &str| {
            return *seen.entry(path.to_string()).or_insert_with(|| {
                let at = Path::new(path);
                let real = at.canonicalize().unwrap_or_else(|_| at.to_path_buf());
                return real.starts_with(&real_repo) && !build_dirs.iter().any(|dir| real.starts_with(dir));
            });
        };
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
            problems.extend(report.problems.into_iter().filter(|problem| return ours(&problem.path)));
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
        let mut functions: HashMap<String, Vec<String>> = HashMap::new();
        let mut test_sites: HashMap<String, BTreeSet<String>> = HashMap::new();
        for use_ in uses {
            if use_.test {
                let keys = match use_.key.strip_prefix("def:test:") {
                    Some(path) => vec![format!("def:lib:{}", path), format!("def:bin:{}", path)],
                    None => vec![use_.key.clone()],
                };
                for key in keys {
                    test_sites.entry(key).or_default().insert(use_.site.clone());
                }
                continue;
            }
            sites.entry(use_.key.clone()).or_default().insert(use_.site.clone());
            let common = functions.entry(use_.key.clone()).or_insert_with(|| use_.functions.clone());
            let shared = common.iter().zip(&use_.functions).take_while(|(a, b)| a == b).count();
            common.truncate(shared);
            let site_path = use_.site.rsplitn(3, ':').last().unwrap_or("");
            if !use_.generated && ours(site_path) {
                written.entry(use_.key).or_default().insert(use_.site);
            }
        }
        let mut judged = BTreeSet::new();
        for def in defs {
            if def.exempt || !ours(&def.path) {
                continue;
            }
            if !judged.insert(def.key.clone()) {
                continue;
            }
            let total = sites.get(&def.key).map(|s| s.len()).unwrap_or(0);
            let by_hand = written.get(&def.key).map(|s| s.len()).unwrap_or(0);
            let home =
                functions
                    .get(&def.key)
                    .filter(|_| def.movable && total == by_hand)
                    .and_then(|common| common.last().cloned());
            let message;
            if total == 0 {
                message = format!("{} `{}` is never used; delete it", def.kind, def.name);
            } else if test_sites
                .get(&def.key)
                .is_some_and(|s| s.iter().any(|site| !sites[&def.key].contains(site))) {
                continue;
            } else if def.inlinable && total == 1 && by_hand == 1 {
                message = format!("{} `{}` is used exactly once; inline it into its one caller", def.kind, def.name);
            } else if let Some(function) = home {
                let function = function.splitn(3, ':').nth(2).unwrap_or(&function).to_string();
                message =
                    format!("{} `{}` is only used in `{}`; move it into that function", def.kind, def.name, function);
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
        return Ok(report(problems, &root));
    })() {
        Ok(code) => exit(code),
        Err(e) => {
            println!("rust-ai-lint: {}", e);
            exit(1);
        },
    }
}
