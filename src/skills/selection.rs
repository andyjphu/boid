use super::{Skill, Skills};
use std::collections::BTreeSet;

pub fn resolve<'a>(skills: &'a Skills, query: &str) -> Result<(&'a str, &'a Skill), String> {
    if (query.starts_with('/') || query.starts_with("bundled:"))
        && let Some(skill) = skills.get(query).filter(|s| s.enabled)
    {
        return Ok((skills.get_key_value(query).unwrap().0.as_str(), skill));
    }
    let matches: Vec<_> = skills
        .iter()
        .filter(|(id, s)| s.enabled && (id.as_str() == query || s.name == query))
        .collect();
    match matches.as_slice() {
        [(id, skill)] => Ok((id.as_str(), skill)),
        [] => Err(format!(
            "Skill {query:?} is unavailable or disabled. Use list_skills."
        )),
        _ => Err(format!(
            "Skill {query:?} is ambiguous. Select a source id from list_skills."
        )),
    }
}

// $name is shorthand; $[source-id] is the unambiguous picker form.
pub fn mentions(text: &str, skills: &Skills) -> Vec<Result<String, String>> {
    let mut result = vec![];
    let mut seen = BTreeSet::new();
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        rest = &rest[pos + 1..];
        let (query, explicit) = if let Some(tail) = rest.strip_prefix('[') {
            let Some(end) = tail.find(']') else {
                continue;
            };
            let query = &tail[..end];
            rest = &tail[end + 1..];
            (query, true)
        } else {
            let end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .unwrap_or(rest.len());
            let query = &rest[..end];
            rest = &rest[end..];
            (query, false)
        };
        if query.is_empty() || !seen.insert(query.to_owned()) {
            continue;
        }
        if !explicit && !skills.iter().any(|(id, s)| id == query || s.name == query) {
            continue;
        }
        result.push(resolve(skills, query).map(|(id, _)| id.to_owned()));
    }
    result
}
