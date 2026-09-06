use {
    crate::Source,
    std::{
        path::Path,
        process::Command,
    },
};

fn run(root: &Path, args: &[&str]) -> Result<String, String> {
    let out =
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|e| format!("failed to run git {:?}: {}", args, e))?;
    if !out.status.success() {
        return Err(format!("git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    return Ok(String::from_utf8_lossy(&out.stdout).to_string());
}

pub fn root(from: &Path) -> Result<std::path::PathBuf, String> {
    let out =
        Command::new("git")
            .arg("-C")
            .arg(from)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|e| format!("failed to run git: {}", e))?;
    if !out.status.success() {
        return Err("not inside a git repository".to_string());
    }
    return Ok(std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()));
}

pub fn rev_exists(root: &Path, rev: &str) -> bool {
    return run(root, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", rev)]).is_ok();
}

fn is_rust(path: &str) -> bool {
    return path.ends_with(".rs");
}

pub fn tree_sources(root: &Path, rev: &str) -> Result<Vec<Source>, String> {
    let listing = run(root, &["ls-tree", "-r", "--name-only", rev])?;
    let mut out = vec![];
    for path in listing.lines() {
        if !is_rust(path) {
            continue;
        }
        let text = run(root, &["show", &format!("{}:{}", rev, path)])?;
        out.push(Source {
            path: path.to_string(),
            text: text,
        });
    }
    return Ok(out);
}

pub fn index_sources(root: &Path) -> Result<Vec<Source>, String> {
    let listing = run(root, &["ls-files", "--cached"])?;
    let mut out = vec![];
    for path in listing.lines() {
        if !is_rust(path) {
            continue;
        }
        let text = match run(root, &["show", &format!(":{}", path)]) {
            Ok(t) => t,
            Err(_) => continue,
        };
        out.push(Source {
            path: path.to_string(),
            text: text,
        });
    }
    return Ok(out);
}

pub fn worktree_sources(root: &Path) -> Result<Vec<Source>, String> {
    let listing = run(root, &["ls-files", "--cached"])?;
    let mut out = vec![];
    for path in listing.lines() {
        if !is_rust(path) {
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
}
