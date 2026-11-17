//! Local skill support. Discovery, presentation and selection can change independently.
pub mod cache;
pub mod catalog;
pub mod discovery;
mod parser;
pub mod selection;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

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
/// Snapshots share the complete map, including every skill body.
/// Discovery mutates only its newly owned map. Persisted JSON stays a plain map.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Skills(Arc<BTreeMap<String, Skill>>);
impl Deref for Skills {
    type Target = BTreeMap<String, Skill>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for Skills {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}
impl PartialEq for Skills {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}
impl From<BTreeMap<String, Skill>> for Skills {
    fn from(value: BTreeMap<String, Skill>) -> Self {
        Self(Arc::new(value))
    }
}

pub fn bundled() -> Skills {
    BTreeMap::from([("bundled:humanizer".into(), Skill {
        name: "humanizer".into(),
        description: "Use when writing or editing prose, including documentation, prompts, tool messages, comments, and explanations. Remove AI writing patterns while preserving meaning, factual claims, and exact quotations.".into(),
        version: "3.1.0".into(),
        source: "Boid bundled copy of Andy's maintained Humanizer; MIT; upstream https://github.com/blader/humanizer".into(),
        text: include_str!("skills/humanizer/SKILL.md").into(),
        directory: None, enabled: true, implicit: true, warnings: vec![],
    })]).into()
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

#[cfg(test)]
mod sharing_tests {
    use super::*;
    #[test]
    fn snapshots_share_bodies_and_preserve_plain_map_json() {
        let original = bundled();
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(bytes, serde_json::to_vec(original.0.as_ref()).unwrap());
        let mut copy = original.clone();
        assert!(Arc::ptr_eq(&original.0, &copy.0));
        assert_eq!(
            original["bundled:humanizer"].text.as_ptr(),
            copy["bundled:humanizer"].text.as_ptr()
        );
        copy.get_mut("bundled:humanizer")
            .unwrap()
            .text
            .push_str("changed");
        assert_ne!(
            original["bundled:humanizer"].text,
            copy["bundled:humanizer"].text
        );
        let restored: Skills = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&restored).unwrap());
        assert!(original == restored);
    }
}
