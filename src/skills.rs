//! Local skill support. Discovery, presentation and selection can change independently.
pub mod catalog;
pub mod discovery;
mod parser;
pub mod selection;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    #[serde(default)]
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: String,
    pub text: String,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub implicit: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}
fn yes() -> bool {
    true
}
pub type Skills = BTreeMap<String, Skill>;

pub fn bundled() -> Skills {
    BTreeMap::from([("bundled:humanizer".into(), Skill {
        name: "humanizer".into(),
        description: "Use when writing or editing prose, including documentation, prompts, tool messages, comments, and explanations. Remove AI writing patterns while preserving meaning, factual claims, and exact quotations.".into(),
        version: "3.1.0".into(),
        source: "Boid bundled copy of Andy's maintained Humanizer; MIT; upstream https://github.com/blader/humanizer".into(),
        text: include_str!("skills/humanizer/SKILL.md").into(),
        directory: None, enabled: true, implicit: true, warnings: vec![],
    })])
}

pub fn instructions(skills: &Skills) -> String {
    format!(
        "{}\n{}",
        include_str!("PRINCIPLES.md"),
        catalog::prompt(skills, 8000, &[])
    )
}

pub fn loaded(id: &str, skill: &Skill) -> Value {
    json!({"id": id, "name": if skill.name.is_empty() { id } else { &skill.name }, "version": skill.version,
        "source": skill.source, "directory":skill.directory, "text":skill.text,
        "warnings":skill.warnings, "truncated":false,
        "resources":"Resolve relative references, scripts and assets against directory. Read or execute only relevant files with ordinary tools. Supporting files are live filesystem content; only this SKILL.md text is captured."})
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
    match selection::resolve(skills, &request.name) {
        Ok((id, skill)) => loaded(id, skill),
        Err(error) => json!({"error":error}),
    }
}
