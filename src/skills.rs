use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
pub struct Skill {
    pub description: String,
    pub version: String,
    pub source: String,
    pub text: String,
}

pub type Skills = BTreeMap<String, Skill>;

pub fn bundled() -> Skills {
    BTreeMap::from([(
        "humanizer".into(),
        Skill {
            description: "Use when writing or editing prose, including documentation, prompts, tool messages, comments, and explanations. Remove AI writing patterns while preserving meaning, factual claims, and exact quotations.".into(),
            version: "3.1.0".into(),
            source: "Boid bundled copy of Andy's maintained Humanizer; MIT; upstream https://github.com/blader/humanizer".into(),
            text: include_str!("skills/humanizer/SKILL.md").into(),
        },
    )])
}

pub fn instructions(skills: &Skills) -> String {
    let mut text = include_str!("PRINCIPLES.md").to_owned();
    text.push_str("\n## Available skills\n\nSkills provide task guidance. When a listed skill applies, use read_skill to read its full text before the relevant work. Reuse it while that text remains in context. Follow the operator's instructions and preserve exact source quotations. Loading a skill does not execute it or establish that its advice was followed.\n");
    for (name, skill) in skills {
        text.push_str(&format!(
            "\n- {name} ({}): {}\n",
            skill.version, skill.description
        ));
    }
    text
}

pub fn read(skills: &Skills, arguments: &str) -> Value {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        name: String,
    }
    let request = match serde_json::from_str::<Request>(arguments) {
        Ok(request) => request,
        Err(error) => return json!({"error":format!("read_skill: {error}")}),
    };
    match skills.get(&request.name) {
        Some(skill) => json!({
            "name":request.name, "version":skill.version, "source":skill.source,
            "text":skill.text, "truncated":false,
        }),
        None => {
            json!({"error":"Skill is not available in this session. Use a name from its startup skill catalog."})
        }
    }
}
