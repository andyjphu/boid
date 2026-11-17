use super::Skills;
use serde_json::{Value, json};

pub fn list(skills: &Skills) -> Value {
    json!(
        skills
            .iter()
            .map(
                |(id, s)| json!({"id":id, "name":if s.name.is_empty() { id } else { &s.name },
        "description":s.description, "version":s.version, "source":s.source,
        "directory":s.directory, "enabled":s.enabled,
        "allow_implicit_invocation":s.implicit, "warnings":s.warnings})
            )
            .collect::<Vec<_>>()
    )
}

pub fn prompt(skills: &Skills, budget: usize, warnings: &[String]) -> String {
    let mut text = include_str!("instructions.md").to_owned();
    let ordered: Vec<_> = skills
        .iter()
        .filter(|(id, _)| id.starts_with("bundled:"))
        .chain(skills.iter().filter(|(id, _)| !id.starts_with("bundled:")))
        .filter(|(_, s)| s.enabled && s.implicit)
        .collect();
    // Names are accepted by read_skill. Only ambiguous names need long source
    // ids in the prompt; list_skills always retains complete source metadata.
    let headers: Vec<_> = ordered
        .iter()
        .map(|(id, skill)| {
            let unique = !skill.name.is_empty()
                && skills
                    .values()
                    .filter(|s| s.enabled && s.name == skill.name)
                    .count()
                    == 1;
            if unique {
                format!("\n- {} ({}): ", skill.name, skill.version)
            } else {
                format!("\n- {} ({}), id={id}: ", skill.name, skill.version)
            }
        })
        .collect();
    let overhead: usize = headers.iter().map(|h| h.chars().count() + 1).sum();
    // The budget is a target, never a reason to hide a skill. Retain a useful
    // description even when the names alone exceed a very small configured limit.
    let allowance = (budget.saturating_sub(overhead) / ordered.len().max(1)).clamp(80, 500);
    let mut used = 0;
    for ((_, skill), header) in ordered.iter().zip(headers) {
        let description = skill
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let description = shorten(&description, allowance);
        let line = format!("{header}{description}\n");
        used += line.chars().count();
        text.push_str(&line);
    }
    if used > budget {
        text.push_str("\nCatalog target exceeded to retain every enabled skill's name and short description.\n");
    }
    if !warnings.is_empty() {
        text.push_str("\nSkill discovery reported errors or unsupported metadata. Use list_skills to inspect them.\n");
    }
    text
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut prefix: String = text.chars().take(limit.saturating_sub(1)).collect();
    if let Some(boundary) = prefix.rfind(' ') {
        // Prefer a whole word without dropping most of a short description.
        if prefix[..boundary].chars().count() >= limit / 2 {
            prefix.truncate(boundary);
        }
    }
    prefix.push('…');
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{bundled, read};

    #[test]
    fn crowded_catalog_keeps_tail_skill_and_readable_descriptions() {
        let mut skills = bundled();
        let template = skills["bundled:humanizer"].clone();
        for n in 0..120 {
            let mut skill = template.clone();
            skill.name = format!("aws-package-{n:03}");
            skill.description =
                "Long package description with Unicode 日本語 and useful task guidance. "
                    .repeat(20);
            skills.insert(format!("/external/packages/{n}/SKILL.md"), skill);
        }
        let mut gmcp = template.clone();
        gmcp.name = "gmcpNOW".into();
        gmcp.description =
            "Find, read and edit existing Google Docs and Google Sheets in Google Drive.".into();
        skills.insert("/zz/gmcpNOW/SKILL.md".into(), gmcp);
        for budget in [0, 8000, 40000] {
            let prompt = prompt(&skills, budget, &[]);
            assert_eq!(
                prompt.lines().filter(|line| line.starts_with("- ")).count(),
                skills.len()
            );
            assert!(prompt.contains("gmcpNOW (3.1.0): Find, read and edit existing Google Docs"));
            assert!(!prompt.contains("skills omitted"));
        }
        assert!(read(&skills, r#"{"name":"gmcpNOW"}"#)["text"].is_string());
        assert_eq!(list(&skills).as_array().unwrap().len(), skills.len());
    }

    #[test]
    fn compact_catalog_respects_policy_and_disambiguates_names() {
        let mut skills = bundled();
        let mut other = skills["bundled:humanizer"].clone();
        skills.insert("/boid/skills/humanizer/SKILL.md".into(), other.clone());
        other.name = "explicit-only".into();
        other.implicit = false;
        skills.insert("explicit".into(), other.clone());
        other.name = "disabled".into();
        other.enabled = false;
        skills.insert("disabled".into(), other);
        let prompt = prompt(&skills, 8000, &[]);
        assert!(prompt.contains("id=bundled:humanizer"));
        assert!(prompt.contains("id=/boid/skills/humanizer/SKILL.md"));
        assert!(!prompt.contains("- explicit-only"));
        assert!(!prompt.contains("- disabled"));
        assert_eq!(list(&skills).as_array().unwrap().len(), 4);
    }
}
