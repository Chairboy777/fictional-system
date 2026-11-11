//! Small text utilities: HTML → plain text, tokenising and light stemming.
//! Kept dependency-free on purpose.

/// Convert the HTML abstracts Cvent returns into readable plain text,
/// keeping paragraph and line breaks.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            // Read the tag name to decide whether it implies a line break.
            let rest = &html[i + 1..];
            let end = rest.find('>').unwrap_or(rest.len());
            let tag = rest[..end].trim_start_matches('/').to_ascii_lowercase();
            let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if matches!(name.as_str(), "p" | "br" | "div" | "li" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                if name == "li" && !rest.starts_with('/') {
                    out.push_str("• ");
                }
            }
            // Skip to the end of the tag.
            for (_, c2) in chars.by_ref() {
                if c2 == '>' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    let decoded = decode_entities(&out);
    // Collapse runs of whitespace within lines and blank lines.
    let mut lines: Vec<String> = Vec::new();
    for line in decoded.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() && lines.last().map_or(true, |p: &String| p.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        if let Some(semi) = after[..after.len().min(12)].find(';') {
            let ent = &after[1..semi];
            let rep = match ent {
                "amp" => Some("&".to_string()),
                "lt" => Some("<".into()),
                "gt" => Some(">".into()),
                "quot" => Some("\"".into()),
                "apos" | "#39" => Some("'".into()),
                "nbsp" => Some(" ".into()),
                "ndash" => Some("–".into()),
                "mdash" => Some("—".into()),
                "rsquo" | "lsquo" => Some("’".into()),
                "ldquo" | "rdquo" => Some("\"".into()),
                "hellip" => Some("…".into()),
                _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32).map(String::from),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32).map(String::from),
                _ => None,
            };
            if let Some(r) = rep {
                out.push_str(&r);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

/// Fold common accented Latin characters so "Müller" matches "muller".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ñ' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ý' | 'ÿ' => 'y',
            '’' | '‘' => '\'',
            c => c,
        })
        .collect()
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "for", "from", "how", "in", "into", "is", "it", "its",
    "of", "on", "or", "our", "that", "the", "their", "this", "to", "we", "what", "when", "which", "while", "who",
    "why", "will", "with", "you", "your", "using", "use", "used", "new", "via", "i", "me", "my", "about", "more",
    "also", "these", "those", "than", "then", "there", "they", "been", "has", "have", "not", "but", "all", "any",
    "such", "each", "other", "over", "within", "across", "through", "paper", "presentation", "session", "talk",
];

/// Normalise one token: fold case/accents, unify British/American "-ise/-ize",
/// and strip simple plural endings.
pub fn stem(tok: &str) -> String {
    let mut t = tok.replace("iz", "is").replace("yz", "ys");
    if t.len() > 4 && t.ends_with("ies") {
        t.truncate(t.len() - 3);
        t.push('y');
    } else if t.len() > 3 && t.ends_with('s') && !t.ends_with("ss") && !t.ends_with("us") && !t.ends_with("is") {
        t.pop();
    }
    t
}

/// Split text into normalised, stemmed tokens with stopwords removed.
/// Single characters are kept only for "r" (the language).
pub fn tokens(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#'))
        .filter(|w| !w.is_empty())
        .filter(|w| w.len() > 1 || *w == "r")
        .filter(|w| !STOPWORDS.contains(w))
        .map(stem)
        .collect()
}

/// Lower-cased, accent-folded text with all non-alphanumerics collapsed to
/// single spaces — used for phrase containment checks.
pub fn normalise_phrase(text: &str) -> String {
    let f = fold(text);
    let mut out = String::with_capacity(f.len() + 2);
    out.push(' ');
    let mut last_space = true;
    for c in f.chars() {
        if c.is_alphanumeric() || c == '+' || c == '#' {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    if !last_space {
        out.push(' ');
    }
    out.replace("iz", "is")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_html() {
        let t = html_to_text("<p>Hello&nbsp;<b>world</b></p><p>Second &amp; last</p>");
        assert_eq!(t, "Hello world\nSecond & last");
    }

    #[test]
    fn stems_spelling_variants() {
        assert_eq!(stem("visualization"), stem("visualisation"));
        assert_eq!(stem("standards"), "standard");
        assert_eq!(tokens("R and Python"), vec!["r", "python"]);
    }
}
