use {
    crate::Source,
    std::{
        path::Path,
        process::Command,
    },
};

pub fn run(root: &Path, args: &[&str]) -> Result<String, String> {
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

pub fn rev_exists(root: &Path, rev: &str) -> bool {
    return run(root, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", rev)]).is_ok();
}

pub fn is_rust(path: &str) -> bool {
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
