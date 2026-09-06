use {
    crate::Source,
    genemichaels_lib::{
        CommentMode,
        WhitespaceMode,
        extract_whitespaces,
    },
    std::collections::HashMap,
};

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct Key {
    pub kind: &'static str,
    pub text: String,
}

pub struct Found {
    pub path: String,
    pub line: usize,
}

pub fn tally(sources: &[Source]) -> (HashMap<Key, usize>, HashMap<Key, Found>, Vec<String>) {
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
