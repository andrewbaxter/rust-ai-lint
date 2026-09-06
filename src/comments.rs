use {
    crate::{
        Problem,
        Source,
    },
    genemichaels_lib::{
        CommentMode,
        WhitespaceMode,
        extract_whitespaces,
    },
    std::collections::HashMap,
};

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Key {
    kind: &'static str,
    text: String,
}

pub struct Found {
    pub path: String,
    pub line: usize,
}

fn tally(sources: &[Source]) -> (HashMap<Key, usize>, HashMap<Key, Found>, Vec<String>) {
    let mut counts: HashMap<Key, usize> = HashMap::new();
    let mut found: HashMap<Key, Found> = HashMap::new();
    let mut errors = vec![];
    for source in sources {
        let whitespaces = match extract_whitespaces(0, &source.text) {
            Ok((w, _)) => w,
            Err(e) => {
                errors.push(format!("{}: parse failed: {}", source.path, e));
                continue;
            },
        };
        for group in whitespaces.values() {
            for whitespace in group {
                let WhitespaceMode::Comment(comment) = &whitespace.mode else {
                    continue;
                };
                let key = Key {
                    kind: match comment.mode {
                        CommentMode::DocInner | CommentMode::DocOuter => "doc",
                        CommentMode::Directive => "directive",
                        CommentMode::Verbatim => "verbatim",
                        CommentMode::ExplicitNormal | CommentMode::Normal => "comment",
                    },
                    text: comment.lines.split_whitespace().collect::<Vec<_>>().join(" "),
                };
                if key.text.is_empty() {
                    continue;
                }
                *counts.entry(key.clone()).or_insert(0) += 1;
                let offset = comment.orig_start_offset.min(source.text.len());
                found.entry(key).or_insert_with(|| Found {
                    path: source.path.clone(),
                    line: source.text[..offset].matches('\n').count() + 1,
                });
            }
        }
    }
    return (counts, found, errors);
}

pub fn check(old: &[Source], new: &[Source], problems: &mut Vec<Problem>) {
    let (old_counts, _, _) = tally(old);
    let (new_counts, new_found, errors) = tally(new);
    for e in errors {
        problems.push(Problem {
            check: "comments",
            path: "".to_string(),
            line: 1,
            message: e,
        });
    }
    let mut reported: Vec<(&Key, usize, usize)> = vec![];
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
