use {
    crate::{
        Problem,
        Source,
    },
    genemichaels_lib::{
        FormatConfig,
        format_str,
    },
    std::path::Path,
};

pub fn load_config(from: &Path) -> Result<FormatConfig, String> {
    let mut at = Some(from);
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
            return serde_json::from_str(&text).map_err(|e| format!("parsing {}: {}", candidate.display(), e));
        }
        at = dir.parent();
    }
    return Ok(FormatConfig::default());
}

pub fn check(sources: &[Source], config: &FormatConfig, problems: &mut Vec<Problem>) {
    for source in sources {
        let res = match format_str(&source.text, config) {
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
                message: format!("genemichaels lost {} comment(s) formatting this file", res.lost_comments.len()),
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
