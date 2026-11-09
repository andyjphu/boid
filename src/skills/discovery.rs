use super::{Skill, Skills, bundled, parser};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub skills: Skills,
    pub warnings: Vec<String>,
    pub budget: usize,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    catalog_characters: Option<usize>,
    #[serde(default)]
    rules: Vec<Rule>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: Option<String>,
    name: Option<String>,
    enabled: Option<bool>,
    allow_implicit_invocation: Option<bool>,
}
#[derive(Default, Deserialize)]
struct Metadata {
    #[serde(default)]
    policy: Policy,
    dependencies: Option<serde_yaml::Value>,
}
#[derive(Default, Deserialize)]
struct Policy {
    allow_implicit_invocation: Option<bool>,
    #[serde(default)]
    products: Vec<String>,
}

pub fn roots(cwd: &Path) -> Vec<PathBuf> {
    let repo = cwd
        .ancestors()
        .find(|p| p.join(".git").exists())
        .unwrap_or(cwd);
    let mut roots = vec![];
    for directory in cwd.ancestors() {
        roots.push(directory.join(".agents/skills"));
        if directory == repo {
            break;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".agents/skills"));
    }
    roots.push(PathBuf::from("/etc/codex/skills"));
    if let Some(paths) = std::env::var_os("BOID_SKILL_PATHS") {
        roots.extend(std::env::split_paths(&paths));
    }
    roots
}

pub fn scan(cwd: &Path) -> Snapshot {
    scan_roots(&roots(cwd), &cwd.join(".boid/skills.json"))
}

pub fn scan_cancellable(cwd: &Path, cancelled: &AtomicBool) -> Option<Snapshot> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    let snapshot = scan_inner(&roots(cwd), &cwd.join(".boid/skills.json"), cancelled);
    (!cancelled.load(Ordering::Acquire)).then_some(snapshot)
}

pub fn scan_roots(roots: &[PathBuf], config_path: &Path) -> Snapshot {
    scan_inner(roots, config_path, &AtomicBool::new(false))
}

fn scan_inner(roots: &[PathBuf], config_path: &Path, cancelled: &AtomicBool) -> Snapshot {
    let mut snapshot = Snapshot {
        skills: bundled(),
        warnings: vec![],
        budget: 8000,
    };
    let config = match read_text(config_path) {
        Ok(text) => serde_json::from_str::<Config>(&text).map_err(anyhow::Error::from),
        Err(error)
            if !config_path.exists()
                && error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(Config::default())
        }
        Err(error) => Err(error),
    };
    let config = match config {
        Ok(config) => config,
        Err(error) => {
            // A broken disable rule must not accidentally enable skills.
            snapshot.skills.clear();
            snapshot.warnings.push(format!(
                "{}: {error}. Skills unavailable until configuration is repaired.",
                config_path.display()
            ));
            return snapshot;
        }
    };
    snapshot.budget = config.catalog_characters.unwrap_or(8000).min(40000);
    let mut visited = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for root in roots {
        walk(
            root,
            0,
            &mut visited,
            &mut paths,
            &mut snapshot.warnings,
            cancelled,
        );
    }
    for path in paths {
        if cancelled.load(Ordering::Acquire) {
            return snapshot;
        }
        match load(&path) {
            Ok(skill) => {
                snapshot
                    .skills
                    .insert(path.to_string_lossy().into_owned(), skill);
            }
            Err(error) => snapshot
                .warnings
                .push(format!("{}: {error}", path.display())),
        }
    }
    for rule in config.rules {
        if rule.id.is_none() == rule.name.is_none() {
            snapshot.skills.clear();
            snapshot.warnings.push("Each skill rule must specify exactly one id or name. Skills unavailable until configuration is repaired.".into());
            break;
        }
        for (id, skill) in &mut snapshot.skills {
            if rule.id.as_ref() == Some(id) || rule.name.as_ref() == Some(&skill.name) {
                if let Some(enabled) = rule.enabled {
                    skill.enabled = enabled;
                }
                if let Some(implicit) = rule.allow_implicit_invocation {
                    skill.implicit = implicit;
                }
            }
        }
    }
    snapshot
}

fn walk(
    path: &Path,
    depth: usize,
    visited: &mut BTreeSet<PathBuf>,
    files: &mut BTreeSet<PathBuf>,
    warnings: &mut Vec<String>,
    cancelled: &AtomicBool,
) {
    if cancelled.load(Ordering::Acquire) {
        return;
    }
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && depth == 0 => return,
        Err(e) => {
            warnings.push(format!("{}: {e}", path.display()));
            return;
        }
    };
    if visited.contains(&canonical) {
        return;
    }
    if depth > 8 || visited.len() >= 1024 || files.len() >= 256 {
        warnings.push(format!(
            "Skill discovery limit reached at {} (8 levels, 1024 directories, 256 packages).",
            path.display()
        ));
        return;
    }
    visited.insert(canonical.clone());
    let skill = canonical.join("SKILL.md");
    if skill.exists() {
        match skill.canonicalize() {
            Ok(p) => {
                files.insert(p);
            }
            Err(e) => warnings.push(format!("{}: {e}", skill.display())),
        }
        return;
    }
    match fs::read_dir(&canonical) {
        Ok(entries) => {
            let mut directories = vec![];
            for entry in entries {
                if cancelled.load(Ordering::Acquire) {
                    return;
                }
                match entry {
                    Ok(entry) if entry.path().is_dir() => directories.push(entry.path()),
                    Ok(_) => (),
                    Err(e) => warnings.push(format!("{}: {e}", canonical.display())),
                }
            }
            directories.sort();
            for directory in directories {
                walk(&directory, depth + 1, visited, files, warnings, cancelled);
            }
        }
        Err(e) => warnings.push(format!("{}: {e}", canonical.display())),
    }
}

fn read_text(path: &Path) -> Result<String> {
    // Check before opening so a FIFO cannot stall the session boundary.
    if !fs::metadata(path)?.is_file() {
        bail!("Expected a regular file");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(512 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 512 * 1024 {
        bail!("File exceeds 512 KiB");
    }
    Ok(String::from_utf8(bytes)?)
}

fn load(path: &Path) -> Result<Skill> {
    let text = read_text(path)?;
    let directory = path.parent().context("Skill has no parent directory")?;
    let parsed = parser::parse_skill_frontmatter_metadata(&text, || {
        directory
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    })?;
    let metadata_path = directory.join("agents/openai.yaml");
    let metadata: Metadata = if metadata_path.exists() {
        serde_yaml::from_str(&read_text(&metadata_path)?)?
    } else {
        Metadata::default()
    };
    let mut warnings = vec![];
    if metadata.dependencies.is_some() {
        warnings.push("Declared dependencies are not provisioned by Boid. Inspect agents/openai.yaml and verify tools before use.".into());
    }
    if !metadata.policy.products.is_empty() {
        warnings.push(
            "Product restrictions in agents/openai.yaml are unsupported; this skill is disabled."
                .into(),
        );
    }
    Ok(Skill {
        name: parsed.name,
        description: parsed.description,
        version: "local file".into(),
        source: path.to_string_lossy().into_owned(),
        text,
        directory: Some(directory.to_string_lossy().into_owned()),
        enabled: metadata.policy.products.is_empty(),
        implicit: metadata.policy.allow_implicit_invocation.unwrap_or(true),
        warnings,
    })
}
