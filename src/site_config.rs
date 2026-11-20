//! Explicit local knowledge and prompt paths. No implicit knowledge-base discovery.
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct SiteConfig {
    pub prompt_path: PathBuf,
    pub prompt: String,
    pub kb: PathBuf,
    pub renderer: PathBuf,
}
impl SiteConfig {
    pub fn load() -> Result<Self> {
        let file = match std::env::var_os("BOID_CONFIG") {
            Some(path) => PathBuf::from(path),
            None => std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
                })
                .join("boid/config.ini"),
        };
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && std::env::var_os("BOID_CONFIG").is_none() =>
            {
                String::new()
            }
            Err(e) => return Err(e).with_context(|| format!("Cannot read {}", file.display())),
        };
        let mut values = parse(&text)?;
        for key in ["DEFAULT_SYSTEM_PROMPT_ADDR", "KB_ADDR", "KB_WIKI_ADDR"] {
            if let Ok(value) = std::env::var(key) {
                values.insert(key.into(), value);
            }
        }
        Self::resolve(&values, file.parent().unwrap_or(Path::new(".")))
    }
    fn resolve(values: &BTreeMap<String, String>, base: &Path) -> Result<Self> {
        let required = |key: &str| -> Result<PathBuf> {
            let value = values.get(key).filter(|v| !v.trim().is_empty())
                .with_context(|| format!("{key} must be set in BOID_CONFIG (default ~/.config/boid/config.ini) or the environment."))?;
            resolve_path(value, base).with_context(|| format!("Invalid {key}"))
        };
        let prompt_path = required("DEFAULT_SYSTEM_PROMPT_ADDR")?;
        let prompt = read_prompt(&prompt_path)?;
        let kb = required("KB_ADDR")?;
        if !kb.is_dir() {
            bail!("KB_ADDR must name a directory: {}", kb.display());
        }
        let renderer = match values.get("KB_WIKI_ADDR") {
            Some(value) => resolve_path(value, base)?,
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("kb-wiki"),
        };
        if !renderer.join("watch-wiki.mjs").is_file() || !renderer.join("node_modules").is_dir() {
            bail!(
                "Initialize kb-wiki with `git submodule update --init --recursive` and `npm ci --prefix kb-wiki`, or set KB_WIKI_ADDR to its installed checkout."
            );
        }
        Ok(Self {
            prompt_path,
            prompt,
            kb,
            renderer,
        })
    }
}
pub fn read_prompt(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read DEFAULT_SYSTEM_PROMPT_ADDR: {}", path.display()))?;
    if text.trim().is_empty() {
        bail!("DEFAULT_SYSTEM_PROMPT_ADDR must point to a nonempty UTF-8 file.");
    }
    Ok(text)
}
fn resolve_path(value: &str, base: &Path) -> Result<PathBuf> {
    let path = if let Some(relative) = value.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").context("HOME is required for ~/ paths.")?)
            .join(relative)
    } else {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    };
    path.canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))
}
fn parse(text: &str) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') || line == "[boid]" {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("Expected KEY=VALUE in Boid config line {}.", index + 1))?;
        let key = key.trim();
        let value = value.trim();
        if !["DEFAULT_SYSTEM_PROMPT_ADDR", "KB_ADDR", "KB_WIKI_ADDR"].contains(&key) {
            bail!("Unknown Boid config key {key}.");
        }
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if result.insert(key.into(), value.into()).is_some() {
            bail!("Duplicate Boid config key {key}.");
        }
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_paths_fail_without_defaults() {
        let e = SiteConfig::resolve(&BTreeMap::new(), Path::new("/tmp"))
            .err()
            .unwrap();
        assert!(
            e.to_string()
                .contains("DEFAULT_SYSTEM_PROMPT_ADDR must be set")
        );
    }
    #[test]
    fn config_preserves_paths_and_rejects_mistakes() {
        let c =
            parse("[boid]\nKB_ADDR = '/work/a b'\nDEFAULT_SYSTEM_PROMPT_ADDR=/work/prompt.md\n")
                .unwrap();
        assert_eq!(c["KB_ADDR"], "/work/a b");
        assert!(parse("KB_ADR=/tmp").is_err());
        assert!(parse("KB_ADDR=a\nKB_ADDR=b").is_err());
        let dir = tempfile::tempdir().unwrap();
        let prompt = dir.path().join("prompt.md");
        std::fs::write(&prompt, " ").unwrap();
        assert!(read_prompt(&prompt).is_err());
        std::fs::write(&prompt, "Role guidance").unwrap();
        assert_eq!(read_prompt(&prompt).unwrap(), "Role guidance");
    }
}
