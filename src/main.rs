use {
    ra_ap_hir::{
        Crate,
        HasAttrs,
        HasVisibility,
        ModuleDef,
        Semantics,
        Visibility,
    },
    ra_ap_ide_db::defs::Definition,
    ra_ap_load_cargo::{
        LoadCargoConfig,
        ProcMacroServerChoice,
        load_workspace_at,
    },
    ra_ap_paths::AbsPathBuf,
    ra_ap_syntax::AstNode,
    ra_ap_project_model::{
        CargoConfig,
        RustLibSource,
    },
    genemichaels_lib::{
        FormatConfig,
        format_str,
    },
    syn::visit::Visit,
    std::{
        collections::{
            HashMap,
            HashSet,
        },
        path::{
            Path,
            PathBuf,
        },
        process::exit,
    },
};

mod comments;
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

fn parsed(
    sources: &[Source],
    report_parse_errors: bool,
    check: &'static str,
    problems: &mut Vec<Problem>,
    mut visit: impl FnMut(&str, &syn::File, &mut Vec<Problem>),
) {
    for source in sources {
        let file = match syn::parse_file(&source.text) {
            Ok(f) => f,
            Err(e) => {
                if report_parse_errors {
                    problems.push(Problem {
                        check: check,
                        path: source.path.clone(),
                        line: e.span().start().line,
                        message: format!("parse failed: {}", e),
                    });
                }
                continue;
            },
        };
        visit(&source.path, &file, problems);
    }
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
        let from = root.unwrap_or_else(|| PathBuf::from("."));
        let from = from.as_path();
        let root = (|| -> Result<PathBuf, String> {
            let out =
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(from)
                    .args(["rev-parse", "--show-toplevel"])
                    .output()
                    .map_err(|e| format!("failed to run git: {}", e))?;
            if !out.status.success() {
                return Err("not inside a git repository".to_string());
            }
            return Ok(std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
        })()?;
        let (old, new) = match &mode {
            Mode::Staged => {
                let old = if git::rev_exists(&root, "HEAD") {
                    git::tree_sources(&root, "HEAD")?
                } else {
                    vec![]
                };
                (old, (|| -> Result<Vec<Source>, String> {
                    let listing = git::run(&root, &["ls-files", "--cached"])?;
                    let mut out = vec![];
                    for path in listing.lines() {
                        if !git::is_rust(path) {
                            continue;
                        }
                        let text = match git::run(&root, &["show", &format!(":{}", path)]) {
                            Ok(t) => t,
                            Err(_) => continue,
                        };
                        out.push(Source {
                            path: path.to_string(),
                            text: text,
                        });
                    }
                    return Ok(out);
                })()?)
            },
            Mode::Worktree => {
                let old = if git::rev_exists(&root, "HEAD") {
                    git::tree_sources(&root, "HEAD")?
                } else {
                    vec![]
                };
                (old, (|| -> Result<Vec<Source>, String> {
                    let listing = git::run(&root, &["ls-files", "--cached"])?;
                    let mut out = vec![];
                    for path in listing.lines() {
                        if !git::is_rust(path) {
                            continue;
                        }
                        let text = match std::fs::read_to_string(root.join(path)) {
                            Ok(t) => t,
                            Err(_) => continue,
                        };
                        out.push(Source {
                            path: path.to_string(),
                            text: text,
                        });
                    }
                    return Ok(out);
                })()?)
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
            let config = (|| -> Result<FormatConfig, String> {
                let mut at = Some(root.as_path());
                while let Some(dir) = at {
                    for name in [".genemichaels.json", "genemichaels.json"] {
                        let candidate = dir.join(name);
                        if !candidate.exists() {
                            continue;
                        }
                        let text =
                            std::fs::read_to_string(
                                &candidate,
                            ).map_err(|e| format!("reading {}: {}", candidate.display(), e))?;
                        return serde_json::from_str(
                            &text,
                        ).map_err(|e| format!("parsing {}: {}", candidate.display(), e));
                    }
                    at = dir.parent();
                }
                return Ok(FormatConfig::default());
            })()?;
            for source in &new {
                let res = match format_str(&source.text, &config) {
                    Ok(r) => r,
                    Err(e) => {
                        problems.push(Problem {
                            check: "format",
                            path: source.path.clone(),
                            line: 1,
                            message: format!("could not be formatted: {}", e),
                        });
                        continue;
                    },
                };
                if !res.lost_comments.is_empty() {
                    problems.push(Problem {
                        check: "format",
                        path: source.path.clone(),
                        line: 1,
                        message: format!(
                            "genemichaels lost {} comment(s) formatting this file",
                            res.lost_comments.len()
                        ),
                    });
                    continue;
                }
                if res.rendered != source.text {
                    problems.push(Problem {
                        check: "format",
                        path: source.path.clone(),
                        line: source
                            .text
                            .lines()
                            .zip(res.rendered.lines())
                            .position(|(a, b)| a != b)
                            .map(|i| i + 1)
                            .unwrap_or_else(|| source.text.lines().count().min(res.rendered.lines().count()) + 1),
                        message: "not genemichaels-formatted; run `genemichaels` on this file".to_string(),
                    });
                }
            }
        }
        if enabled.contains("comments") {
            if !old.is_empty() {
                let (old_counts, _, _) = comments::tally(&old);
                let (new_counts, new_found, errors) = comments::tally(&new);
                for e in errors {
                    problems.push(Problem {
                        check: "comments",
                        path: "".to_string(),
                        line: 1,
                        message: e,
                    });
                }
                let mut reported: Vec<(&comments::Key, usize, usize)> = vec![];
                for (key, new_count) in &new_counts {
                    let old_count = old_counts.get(key).copied().unwrap_or(0);
                    if *new_count > old_count {
                        reported.push((key, old_count, *new_count));
                    }
                }
                reported.sort_by(|a, b| a.0.text.cmp(&b.0.text));
                for (key, old_count, new_count) in reported {
                    let where_ = new_found.get(key);
                    let verb = if old_count == 0 {
                        "added"
                    } else {
                        "duplicated"
                    };
                    const LIMIT: usize = 70;
                    let text = if key.text.chars().count() <= LIMIT {
                        key.text.clone()
                    } else {
                        format!("{}...", key.text.chars().take(LIMIT).collect::<String>())
                    };
                    problems.push(Problem {
                        check: "comments",
                        path: where_.map(|f| f.path.clone()).unwrap_or_default(),
                        line: where_.map(|f| f.line).unwrap_or(1),
                        message: format!(
                            "{} {} (appeared {} time(s) before, {} now): {}",
                            verb,
                            key.kind,
                            old_count,
                            new_count,
                            text
                        ),
                    });
                }
            }
        }
        if enabled.contains("naming") {
            parsed(&new, true, "naming", &mut problems, |path, file, problems| {
                let mut checker = naming::Checker {
                    path: path,
                    problems: problems,
                };
                checker.visit_file(file);
            });
        }
        if enabled.contains("returns") {
            parsed(&new, false, "returns", &mut problems, |path, file, problems| {
                let mut checker = returns::Checker {
                    path: path,
                    problems: problems,
                };
                checker.visit_file(file);
            });
        }
        if enabled.contains("shorthand") {
            parsed(&new, false, "shorthand", &mut problems, |path, file, problems| {
                let mut checker = shorthand::Checker {
                    path: path,
                    problems: problems,
                };
                checker.visit_file(file);
            });
        }
        if enabled.contains("suppressions") {
            parsed(&new, false, "suppressions", &mut problems, |path, file, problems| {
                let mut checker = suppressions::Checker {
                    path: path,
                    problems: problems,
                };
                checker.visit_file(file);
            });
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
                let cargo_root = cargo_root.as_path();
                let found_src = (|| -> Option<std::path::PathBuf> {
                    if let Some(dir) = rust_src.as_deref() {
                        if usage::is_library_dir(dir) {
                            return Some(dir.to_path_buf());
                        }
                        return None;
                    }
                    if let Ok(dir) = std::env::var("RUST_SRC_PATH") {
                        let dir = std::path::PathBuf::from(dir);
                        if usage::is_library_dir(&dir) {
                            return Some(dir);
                        }
                    }
                    let out = match std::process::Command::new("rustc").arg("--print").arg("sysroot").output() {
                        Ok(o) => o,
                        Err(_) => return None,
                    };
                    if !out.status.success() {
                        return None;
                    }
                    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let dir = Path::new(&sysroot).join("lib/rustlib/src/rust/library");
                    if usage::is_library_dir(&dir) {
                        return Some(dir);
                    }
                    return None;
                })();
                let Some(rust_src) = found_src else {
                    return Err(
                        [
                            "the usage check needs the standard library source, without which reference counts are wrong",
                            "supply it with `rustup component add rust-src`, by setting RUST_SRC_PATH, or with --rust-src <dir>",
                            "on nix: nix-build '<nixpkgs>' -A rustPlatform.rustLibSrc --no-out-link",
                        ].join("\n  "),
                    );
                };
                let mut cargo_config = CargoConfig::default();
                cargo_config.all_targets = true;
                cargo_config.set_test = true;
                cargo_config.sysroot = Some(RustLibSource::Discover);
                let absolute = std::fs::canonicalize(&rust_src).unwrap_or(rust_src);
                cargo_config.sysroot_src = Some(AbsPathBuf::assert_utf8(absolute));
                let load_config = LoadCargoConfig {
                    load_out_dirs_from_check: true,
                    with_proc_macro_server: ProcMacroServerChoice::Sysroot,
                    prefill_caches: false,
                    num_worker_threads: 1,
                    proc_macro_processes: 1,
                };
                let (db, vfs, proc_macro) =
                    load_workspace_at(
                        cargo_root,
                        &cargo_config,
                        &load_config,
                        &|_| { },
                    ).map_err(|e| format!("failed to load the cargo workspace at {}: {}", cargo_root.display(), e))?;
                let derives_expand = proc_macro.is_some();
                if !derives_expand {
                    eprintln!(
                        "note: no proc-macro server available, so derive-generated uses are invisible; skipping fields"
                    );
                }
                let found = ra_ap_hir::attach_db(&db, || {
                    let db = &db;
                    let vfs = &vfs;
                    let mut problems = vec![];
                    let sema = Semantics::new(db);
                    let mut locals: Vec<Definition> = vec![];
                    let counts = {
                        let mut counts = HashMap::new();
                        for (file_id, path) in vfs.iter() {
                            let Some(path) = path.as_path() else {
                                continue;
                            };
                            if !path.as_str().ends_with(".rs") {
                                continue;
                            }
                            if !sema.file_to_module_defs(file_id).any(|m| m.krate(db).origin(db).is_local()) {
                                continue;
                            }
                            let source = sema.parse(sema.attach_first_edition(file_id));
                            usage::tally(&sema, source.syntax(), 0, &mut counts, &mut locals);
                        }
                        counts
                    };
                    let repo_root = root.as_path();
                    let binary_crates: Vec<Crate> =
                        Crate::all(db).into_iter().filter(|k| k.origin(db).is_local()).filter(|k| {
                            return k.root_module(db).declarations(db).into_iter().any(|d| {
                                let ModuleDef::Function(f) = d else {
                                    return false;
                                };
                                return f.name(db).as_str() == "main";
                            });
                        }).collect();
                    let mut defs: Vec<Definition> = locals;
                    for krate in Crate::all(db) {
                        if !krate.origin(db).is_local() {
                            continue;
                        }
                        for module in krate.modules(db) {
                            for decl in module.declarations(db) {
                                match decl {
                                    ModuleDef::Module(_) => continue,
                                    ModuleDef::Adt(adt) => {
                                        defs.push(Definition::Adt(adt));
                                        if let ra_ap_hir::Adt::Struct(s) = adt {
                                            let derived =
                                                usage::locate(&sema, vfs, repo_root, Definition::Adt(adt))
                                                    .map(
                                                        |(_, text)| text
                                                            .lines()
                                                            .any(|l| l.trim_start().starts_with("#[derive")),
                                                    )
                                                    .unwrap_or(true);
                                            if !derived {
                                                for field in s.fields(db) {
                                                    defs.push(Definition::Field(field));
                                                }
                                            }
                                        }
                                    },
                                    other => defs.push(Definition::from(other)),
                                }
                            }
                            for imp in module.impl_defs(db) {
                                if imp.trait_(db).is_some() {
                                    continue;
                                }
                                for item in imp.items(db) {
                                    defs.push(Definition::from(item));
                                }
                            }
                        }
                    }
                    for def in defs {
                        let Some(name) = def.name(db) else {
                            continue;
                        };
                        let name = name.as_str().to_string();
                        if name == "main" {
                            continue;
                        }
                        if name.chars().all(|c| c.is_ascii_digit()) {
                            continue;
                        }
                        if matches!(def, Definition::Local(_)) && name.starts_with('_') {
                            continue;
                        }
                        if let Definition::Function(f) = def {
                            if f.attrs(db).is_test() {
                                continue;
                            }
                        }
                        let ignore_pub =
                            def.module(db).map(|m| binary_crates.contains(&m.krate(db))).unwrap_or(false);
                        if !ignore_pub {
                            let vis = match def {
                                Definition::Field(f) => Some(f.visibility(db)),
                                Definition::Function(f) => Some(f.visibility(db)),
                                Definition::Adt(a) => Some(a.visibility(db)),
                                Definition::Const(c) => Some(c.visibility(db)),
                                Definition::Static(s) => Some(s.visibility(db)),
                                Definition::TypeAlias(t) => Some(t.visibility(db)),
                                Definition::Trait(t) => Some(t.visibility(db)),
                                _ => None,
                            };
                            let visible = matches!(vis, Some(Visibility::Public));
                            if visible {
                                continue;
                            }
                        }
                        let Some((where_, item_text)) = usage::locate(&sema, &vfs, repo_root, def) else {
                            continue;
                        };
                        let mut is_test = false;
                        for line in item_text.lines() {
                            let line = line.trim();
                            if !line.starts_with("#[") {
                                if !line.is_empty() && !line.starts_with("//") {
                                    break;
                                }
                                continue;
                            }
                            if line.ends_with("test]") || line.contains("(test)") || line.ends_with("bench]") {
                                is_test = true;
                                break;
                            }
                        }
                        if is_test {
                            continue;
                        }
                        if item_text.lines().any(|l| l.trim_start().starts_with("#[proc_macro")) {
                            continue;
                        }
                        if let Definition::Function(_) = def {
                            if !item_text.contains(&format!("fn {}", name)) {
                                continue;
                            }
                        }
                        let count = counts.get(&def).copied().unwrap_or(0);
                        let message = match count {
                            0 => format!("{} `{}` is never used", usage::describe(db, def), name),
                            1 => {
                                match def {
                                    Definition::Function(_) | Definition::Const(_) | Definition::Static(_) => { },
                                    _ => continue,
                                }
                                format!(
                                    "{} `{}` is used exactly once; consider inlining it",
                                    usage::describe(db, def),
                                    name
                                )
                            },
                            _ => continue,
                        };
                        problems.push(Problem {
                            check: "usage",
                            path: where_.path,
                            line: where_.line,
                            message: message,
                        });
                    }
                    return problems;
                });
                problems.extend(found);
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
