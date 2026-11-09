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
    let mut used = 0;
    let mut omitted = 0;
    let ordered = skills
        .iter()
        .filter(|(id, _)| id.starts_with("bundled:"))
        .chain(skills.iter().filter(|(id, _)| !id.starts_with("bundled:")));
    for (id, skill) in ordered.filter(|(_, s)| s.enabled && s.implicit) {
        let description: String = skill.description.chars().take(500).collect();
        let line = format!(
            "\n- {} ({}), id={id}: {description}\n",
            skill.name, skill.version
        );
        let size = line.chars().count();
        if used + size > budget {
            omitted += 1;
            continue;
        }
        text.push_str(&line);
        used += size;
    }
    if omitted > 0 {
        text.push_str(&format!("\n{omitted} skills omitted from this catalog's character budget. Use list_skills to discover them.\n"));
    }
    if !warnings.is_empty() {
        text.push_str("\nSkill discovery reported errors or unsupported metadata. Use list_skills to inspect them.\n");
    }
    text
}
