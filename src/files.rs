use std::fs::{self, File, Permissions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::{ExecOutput, OUTPUT_LIMIT, Output, Tools};

const EDIT_LIMIT: usize = 1024 * 1024;

impl Tools {
    /// List matching files. Explicit globs override ripgrep's ignore rules.
    pub async fn glob(&self, pattern: &str) -> io::Result<ExecOutput> {
        let glob = format!("--glob={pattern}");
        self.run(
            "rg",
            &["--files", "--sort=path", &glob, "--", "."],
            Duration::from_secs(30),
        )
        .await
    }

    /// Search file contents with a ripgrep regular expression and optional file glob.
    pub async fn search(&self, pattern: &str, glob: Option<&str>) -> io::Result<ExecOutput> {
        let glob = glob.map(|pattern| format!("--glob={pattern}"));
        let mut args = vec![
            "--line-number",
            "--with-filename",
            "--color=never",
            "--sort=path",
        ];
        if let Some(glob) = glob.as_deref() {
            args.push(glob);
        }
        args.extend(["--", pattern, "."]);
        self.run("rg", &args, Duration::from_secs(30)).await
    }

    /// Read a regular file. Output reports when only a prefix fits the output limit.
    pub fn read(&self, path: impl AsRef<Path>) -> io::Result<Output> {
        let path = self.path(path);
        if !fs::metadata(&path)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Read requires a regular file.",
            ));
        }
        let mut bytes = Vec::new();
        File::open(path)?
            .take((OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        Ok(Output::from_bytes(&bytes))
    }

    /// Replace a file atomically. The parent must exist; symlinks are rejected.
    pub fn write(&self, path: impl AsRef<Path>, content: &str) -> io::Result<()> {
        let path = self.path(path);
        let permissions = replacement_permissions(&path)?;
        replace(&path, content, permissions)
    }

    /// Replace one exact occurrence. Ambiguous, absent, and empty matches leave the file unchanged.
    pub fn edit(&self, path: impl AsRef<Path>, old: &str, new: &str) -> io::Result<()> {
        if old.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Edit requires nonempty old text.",
            ));
        }
        let path = self.path(path);
        let permissions = replacement_permissions(&path)?;
        let mut bytes = Vec::new();
        File::open(&path)?
            .take((EDIT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > EDIT_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Edit supports files up to 1 MiB.",
            ));
        }
        let mut content = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let start = content.find(old).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Old text was not found; file unchanged.",
            )
        })?;
        if content.rfind(old) != Some(start) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Old text is ambiguous; file unchanged.",
            ));
        }
        content.replace_range(start..start + old.len(), new);
        replace(&path, &content, permissions)
    }
}

fn replacement_permissions(path: &Path) -> io::Result<Option<Permissions>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.permissions())),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Replace requires a regular file or a new path. Use a symlink's target path explicitly.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn replace(path: &Path, content: &str, permissions: Option<Permissions>) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "File path has no parent directory.",
        )
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_edit_preconditions_preserve_the_file() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let tools = Tools::new(directory.path())?;
        let path = directory.path().join("example.txt");
        for (content, old) in [
            ("alpha", ""),
            ("alpha", "missing"),
            ("alpha alpha", "alpha"),
            ("aaa", "aa"),
        ] {
            fs::write(&path, content)?;
            assert!(tools.edit("example.txt", old, "changed").is_err());
            assert_eq!(fs::read_to_string(&path)?, content);
        }
        Ok(())
    }

    #[test]
    fn edits_preserve_surrounding_text_and_permissions() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let tools = Tools::new(directory.path())?;
        let path = directory.path().join("script.sh");
        tools.write("script.sh", "before\r\nold\r\nafter\r\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, Permissions::from_mode(0o751))?;
        }
        tools.edit("script.sh", "old", "new")?;
        assert_eq!(fs::read_to_string(&path)?, "before\r\nnew\r\nafter\r\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o751);
        }
        Ok(())
    }

    #[test]
    fn read_reports_truncation_and_rejects_directories() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let tools = Tools::new(directory.path())?;
        fs::write(
            directory.path().join("large.txt"),
            vec![b'x'; OUTPUT_LIMIT + 10],
        )?;
        let output = tools.read("large.txt")?;
        assert!(output.truncated);
        assert_eq!(output.text.len(), OUTPUT_LIMIT);
        assert!(tools.read(".").is_err());
        Ok(())
    }

    #[tokio::test]
    async fn searches_respect_ignore_rules_and_literal_option_boundaries() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let tools = Tools::new(directory.path())?;
        fs::create_dir(directory.path().join("src"))?;
        fs::write(
            directory.path().join("src/lib.rs"),
            "let value = -needle;\n",
        )?;
        fs::write(directory.path().join("ignored.rs"), "-needle\n")?;
        fs::write(directory.path().join(".ignore"), "ignored.rs\n")?;
        let listed = tools.glob("**/*.rs").await?;
        assert!(listed.status.success());
        assert_eq!(listed.stdout.text.lines().count(), 2);
        assert!(listed.stdout.text.contains("src/lib.rs"));
        assert!(listed.stdout.text.contains("ignored.rs"));
        let found = tools.search("-needle", None).await?;
        assert!(found.status.success());
        assert_eq!(found.stdout.text.lines().count(), 1);
        assert!(
            found
                .stdout
                .text
                .contains("src/lib.rs:1:let value = -needle;")
        );
        let explicit = tools.search("-needle", Some("*.rs")).await?;
        assert!(explicit.status.success());
        assert_eq!(explicit.stdout.text.lines().count(), 2);
        Ok(())
    }
}
