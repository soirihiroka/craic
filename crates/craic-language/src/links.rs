#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File {
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
}

pub fn detected_links(text: &str) -> Vec<(usize, usize, LinkTarget)> {
    let mut links = Vec::new();
    let mut token_start = None;
    for (index, character) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if character.is_whitespace() {
            if let Some(start) = token_start.take()
                && let Some(link) = detected_token(text, start, index)
            {
                links.push(link);
            }
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }
    links
}

pub fn destination_target(destination: &str) -> LinkTarget {
    if destination.starts_with("https://")
        || destination.starts_with("http://")
        || destination.starts_with("mailto:")
    {
        LinkTarget::Url(destination.to_owned())
    } else if let Some(target) = parse_file_target(destination) {
        target
    } else {
        LinkTarget::Url(destination.to_owned())
    }
}

fn detected_token(
    text: &str,
    token_start: usize,
    token_end: usize,
) -> Option<(usize, usize, LinkTarget)> {
    let token = &text[token_start..token_end];
    let leading = token
        .trim_start_matches(['(', '[', '{', '<', '\'', '"'])
        .len();
    let leading = token.len() - leading;
    let candidate = &token[leading..];
    let trimmed =
        candidate.trim_end_matches(['.', ',', ';', '!', '?', ')', ']', '}', '>', '\'', '"']);
    if trimmed.is_empty() {
        return None;
    }
    let start = token_start + leading;
    let end = start + trimmed.len();
    if trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("mailto:")
    {
        return Some((start, end, LinkTarget::Url(trimmed.to_owned())));
    }
    parse_file_target(trimmed).map(|target| (start, end, target))
}

fn parse_file_target(candidate: &str) -> Option<LinkTarget> {
    let candidate = candidate.strip_prefix("file://").unwrap_or(candidate);
    if candidate.is_empty() || candidate.contains("://") {
        return None;
    }
    let (path, line, column) = split_location(candidate);
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let known_extension = extension.as_deref().is_some_and(|extension| {
        matches!(
            extension,
            "rs" | "toml"
                | "md"
                | "txt"
                | "json"
                | "jsonc"
                | "yaml"
                | "yml"
                | "xml"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "py"
                | "rb"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "hpp"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "sql"
                | "csv"
                | "log"
                | "diff"
                | "patch"
                | "ini"
                | "conf"
                | "config"
                | "lock"
                | "svg"
        )
    });
    let path_like = path.starts_with('/')
        || path.starts_with("./")
        || path.starts_with("../")
        || path.contains('/')
        || known_extension;
    if !path_like || path.ends_with('/') {
        return None;
    }
    Some(LinkTarget::File {
        path: path.to_owned(),
        line,
        column,
    })
}

fn split_location(candidate: &str) -> (&str, Option<usize>, Option<usize>) {
    if let Some((path, fragment)) = candidate.rsplit_once("#L")
        && !path.is_empty()
    {
        let start = fragment.split('-').next().unwrap_or(fragment);
        if let Some((line, column)) = start.split_once('C')
            && let (Ok(line), Ok(column)) = (line.parse::<usize>(), column.parse::<usize>())
            && line > 0
            && column > 0
        {
            return (path, Some(line), Some(column));
        }
        if let Ok(line) = start.parse::<usize>()
            && line > 0
        {
            return (path, Some(line), None);
        }
    }
    let Some((before_last, last)) = candidate.rsplit_once(':') else {
        return (candidate, None, None);
    };
    let Ok(last) = last.parse::<usize>() else {
        return (candidate, None, None);
    };
    if last == 0 {
        return (candidate, None, None);
    }
    if let Some((path, line)) = before_last.rsplit_once(':')
        && let Ok(line) = line.parse::<usize>()
        && line > 0
    {
        return (path, Some(line), Some(last));
    }
    (before_last, Some(last), None)
}
