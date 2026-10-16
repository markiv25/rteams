//! SKILL.md frontmatter: a small YAML-subset parser (scalars, quoted strings, inline and block
//! lists, `|` / `>` block scalars) and a writer. Unknown keys are preserved in order.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    List(Vec<String>),
}

impl Value {
    pub fn as_string(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::List(l) => l.join(", "),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frontmatter {
    pub fields: Vec<(String, Value)>,
    /// The file had an opening `---` but no closing one, or a line we could not parse.
    pub malformed: bool,
    /// The file has a frontmatter block at all.
    pub present: bool,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).map(Value::as_string)
    }
}

/// Typed view of the fields the app cares about.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillMeta {
    pub name: Option<String>,
    pub description: Option<String>,
    pub disable_model_invocation: bool,
    pub allowed_tools: Vec<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    /// Other keys, kept verbatim.
    pub extra: Vec<(String, Value)>,
}

impl SkillMeta {
    pub fn from_frontmatter(fm: &Frontmatter) -> SkillMeta {
        let mut m = SkillMeta::default();
        for (k, v) in &fm.fields {
            match k.as_str() {
                "name" => m.name = Some(v.as_string()),
                "description" => m.description = Some(v.as_string()),
                "license" => m.license = Some(v.as_string()),
                "compatibility" => m.compatibility = Some(v.as_string()),
                "disable-model-invocation" => {
                    m.disable_model_invocation =
                        matches!(v.as_string().to_lowercase().as_str(), "true" | "yes" | "on")
                }
                "allowed-tools" => {
                    m.allowed_tools = match v {
                        Value::List(l) => l.clone(),
                        Value::Str(s) => s
                            .split([',', ' '])
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .collect(),
                    }
                }
                _ => m.extra.push((k.clone(), v.clone())),
            }
        }
        m
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        let inner = &s[1..s.len() - 1];
        if b[0] == b'"' {
            return inner
                .replace("\\\"", "\"")
                .replace("\\n", "\n")
                .replace("\\\\", "\\");
        }
        return inner.replace("''", "'");
    }
    // strip trailing comment
    if let Some(i) = s.find(" #") {
        return s[..i].trim().to_string();
    }
    s.to_string()
}

fn inline_list(s: &str) -> Vec<String> {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(unquote)
        .filter(|x| !x.is_empty())
        .collect()
}

/// Split a document into (frontmatter, body). The body never includes the `---` fences.
pub fn split(text: &str) -> (Frontmatter, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    match lines.next() {
        Some(l) if l.trim_end() == "---" => {}
        _ => return (Frontmatter::default(), text.to_string()),
    }
    let all: Vec<&str> = text.lines().collect();
    let Some(end) =
        (1..all.len()).find(|&i| all[i].trim_end() == "---" || all[i].trim_end() == "...")
    else {
        return (
            Frontmatter {
                fields: vec![],
                malformed: true,
                present: true,
            },
            text.to_string(),
        );
    };
    let mut fm = Frontmatter {
        present: true,
        ..Default::default()
    };
    let block = &all[1..end];
    let mut i = 0;
    while i < block.len() {
        let line = block[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            fm.malformed = true;
            continue;
        }
        let Some((k, rest)) = line.split_once(':') else {
            fm.malformed = true;
            continue;
        };
        let key = k.trim().to_string();
        let rest = rest.trim();
        let value = if rest == "|" || rest == ">" || rest == "|-" || rest == ">-" {
            let folded = rest.starts_with('>');
            let mut parts = vec![];
            while i < block.len()
                && (block[i].starts_with(' ')
                    || block[i].starts_with('\t')
                    || block[i].trim().is_empty())
            {
                parts.push(block[i].trim());
                i += 1;
            }
            while parts.last() == Some(&"") {
                parts.pop();
            }
            Value::Str(parts.join(if folded { " " } else { "\n" }))
        } else if rest.is_empty() {
            let mut items = vec![];
            let mut cont = vec![];
            while i < block.len()
                && (block[i].starts_with(' ')
                    || block[i].starts_with('\t')
                    || block[i].trim_start().starts_with("- "))
            {
                let t = block[i].trim();
                if let Some(item) = t.strip_prefix("- ") {
                    items.push(unquote(item));
                } else if !t.is_empty() {
                    cont.push(t.to_string());
                }
                i += 1;
            }
            if !items.is_empty() {
                Value::List(items)
            } else {
                Value::Str(cont.join(" "))
            }
        } else if rest.starts_with('[') {
            Value::List(inline_list(rest))
        } else {
            // plain multi-line scalar continuation
            let mut s = unquote(rest);
            while i < block.len()
                && (block[i].starts_with("  ") || block[i].starts_with('\t'))
                && !block[i].trim_start().starts_with("- ")
            {
                s.push(' ');
                s.push_str(block[i].trim());
                i += 1;
            }
            Value::Str(s)
        };
        fm.fields.push((key, value));
    }
    let body = all[end + 1..].join("\n");
    let body = body.strip_prefix('\n').unwrap_or(&body).to_string();
    (fm, body)
}

pub fn parse(text: &str) -> (SkillMeta, String, Frontmatter) {
    let (fm, body) = split(text);
    (SkillMeta::from_frontmatter(&fm), body, fm)
}

fn quote_if_needed(s: &str) -> String {
    let needs = s.is_empty()
        || s.contains(": ")
        || s.contains(" #")
        || s.starts_with([
            '[', '{', '"', '\'', '-', '*', '&', '!', '|', '>', '%', '@', '`', '#',
        ])
        || s.ends_with(':')
        || s.trim() != s
        || matches!(
            s.to_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "~"
        );
    if needs {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    } else {
        s.to_string()
    }
}

/// Render `meta` + `body` back into SKILL.md text.
pub fn render(meta: &SkillMeta, body: &str) -> String {
    let mut out = String::from("---\n");
    if let Some(n) = &meta.name {
        out.push_str(&format!("name: {}\n", quote_if_needed(n)));
    }
    if let Some(d) = &meta.description {
        out.push_str(&format!("description: {}\n", quote_if_needed(d)));
    }
    if let Some(l) = &meta.license {
        if !l.is_empty() {
            out.push_str(&format!("license: {}\n", quote_if_needed(l)));
        }
    }
    if let Some(c) = &meta.compatibility {
        if !c.is_empty() {
            out.push_str(&format!("compatibility: {}\n", quote_if_needed(c)));
        }
    }
    if meta.disable_model_invocation {
        out.push_str("disable-model-invocation: true\n");
    }
    if !meta.allowed_tools.is_empty() {
        out.push_str(&format!(
            "allowed-tools: {}\n",
            meta.allowed_tools.join(", ")
        ));
    }
    for (k, v) in &meta.extra {
        match v {
            Value::Str(s) => out.push_str(&format!("{k}: {}\n", quote_if_needed(s))),
            Value::List(l) => {
                out.push_str(&format!("{k}:\n"));
                for i in l {
                    out.push_str(&format!("  - {}\n", quote_if_needed(i)));
                }
            }
        }
    }
    out.push_str("---\n\n");
    out.push_str(body.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars_lists_and_blocks() {
        let t = "---\nname: my-skill\ndescription: \"Does: things\"\nallowed-tools: Read, Bash\ndisable-model-invocation: true\ntags: [a, 'b']\nlong: >\n  one\n  two\nitems:\n  - x\n  - y\n---\n# Body\n";
        let (m, body, fm) = parse(t);
        assert_eq!(m.name.as_deref(), Some("my-skill"));
        assert_eq!(m.description.as_deref(), Some("Does: things"));
        assert_eq!(m.allowed_tools, vec!["Read", "Bash"]);
        assert!(m.disable_model_invocation);
        assert_eq!(
            fm.get("tags"),
            Some(&Value::List(vec!["a".into(), "b".into()]))
        );
        assert_eq!(fm.get_str("long").unwrap(), "one two");
        assert_eq!(
            fm.get("items"),
            Some(&Value::List(vec!["x".into(), "y".into()]))
        );
        assert_eq!(body, "# Body");
        assert!(!fm.malformed);
    }

    #[test]
    fn missing_or_unclosed_frontmatter() {
        let (fm, body) = split("# just text");
        assert!(!fm.present);
        assert_eq!(body, "# just text");
        let (fm, _) = split("---\nname: x\nno end");
        assert!(fm.malformed && fm.present);
    }

    #[test]
    fn render_round_trips() {
        let m = SkillMeta {
            name: Some("a-b".into()),
            description: Some("Use when: x".into()),
            allowed_tools: vec!["Read".into()],
            disable_model_invocation: true,
            ..Default::default()
        };
        let text = render(&m, "Body\n");
        let (m2, body, _) = parse(&text);
        assert_eq!(m2, m);
        assert_eq!(body, "Body\n".trim_end());
    }
}
