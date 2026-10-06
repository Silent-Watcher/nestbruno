//! Project discovery: locating the project root and loading its configuration.

use std::path::{Path, PathBuf};

use crate::config::{CONFIG_FILE_NAME, Config};
use crate::error::Error;

/// A NestJS project with its resolved configuration.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    config: Config,
    config_path: Option<PathBuf>,
}

impl Project {
    /// Open the project rooted at `root`.
    ///
    /// The root is canonicalized once here; all later paths are derived from it.
    /// A missing `nestbruno.toml` is not an error: defaults apply.
    pub fn open(root: &Path) -> Result<Self, Error> {
        let root = root.canonicalize().map_err(|source| Error::Io {
            path: root.to_owned(),
            source,
        })?;
        let (config_path, config) = match Config::load(&root)? {
            Some((path, config)) => (Some(path), config),
            None => (None, Config::default()),
        };
        Ok(Self {
            root,
            config,
            config_path,
        })
    }

    /// Find the project root by walking up from `start`.
    ///
    /// A directory containing `nestbruno.toml` wins over one that only has a
    /// `package.json`, so a monorepo-level config is preferred over the
    /// `package.json` of a nested package.
    pub fn find_root(start: &Path) -> Result<PathBuf, Error> {
        for marker in [CONFIG_FILE_NAME, "package.json"] {
            if let Some(dir) = start.ancestors().find(|dir| dir.join(marker).is_file()) {
                return Ok(dir.to_owned());
            }
        }
        Err(Error::ProjectNotFound {
            start: start.to_owned(),
        })
    }

    /// Canonical absolute project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configuration file that was loaded, or `None` when defaults apply.
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// Absolute path of the configured source directory (not checked for existence).
    pub fn source_root(&self) -> PathBuf {
        self.root.join(&self.config.source)
    }

    /// Absolute path of the configured Bruno collection directory.
    pub fn bruno_root(&self) -> PathBuf {
        self.root.join(&self.config.bruno)
    }

    /// Fail unless the source directory exists and is a directory.
    pub fn ensure_source_dir(&self) -> Result<(), Error> {
        let path = self.source_root();
        if path.is_dir() {
            Ok(())
        } else {
            Err(Error::SourceDirMissing { path })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn open_without_config_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert_eq!(project.config(), &Config::default());
        assert!(project.config_path().is_none());
    }

    #[test]
    fn open_with_config_loads_it() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CONFIG_FILE_NAME), "source = \"lib\"").unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert_eq!(project.config().source, PathBuf::from("lib"));
        assert!(project.config_path().is_some());
        assert_eq!(project.source_root(), project.root().join("lib"));
    }

    #[test]
    fn open_missing_directory_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = Project::open(&dir.path().join("missing")).unwrap_err();
        assert!(matches!(err, Error::Io { .. }));
    }

    #[test]
    fn find_root_prefers_nestbruno_toml_over_nested_package_json() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("packages/api/src");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join(CONFIG_FILE_NAME), "").unwrap();
        fs::write(dir.path().join("packages/api/package.json"), "{}").unwrap();
        assert_eq!(Project::find_root(&nested).unwrap(), dir.path());
    }

    #[test]
    fn find_root_falls_back_to_package_json() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(Project::find_root(&nested).unwrap(), dir.path());
    }

    #[test]
    fn ensure_source_dir_reports_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert!(matches!(
            project.ensure_source_dir().unwrap_err(),
            Error::SourceDirMissing { .. }
        ));
        fs::create_dir(dir.path().join("src")).unwrap();
        project.ensure_source_dir().unwrap();
    }

    #[test]
    fn ensure_source_dir_rejects_a_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("src"), "").unwrap();
        let project = Project::open(dir.path()).unwrap();
        assert!(project.ensure_source_dir().is_err());
    }
}
