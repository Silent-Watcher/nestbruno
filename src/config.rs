//! The `nestbruno.toml` configuration model.
//!
//! Every key is optional. Unknown keys are rejected so that typos surface
//! immediately instead of being silently ignored.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::error::Error;

/// Name of the configuration file, looked up in the project root.
pub const CONFIG_FILE_NAME: &str = "nestbruno.toml";

/// Configuration files larger than this are rejected (resource exhaustion).
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Contents written by `nestbruno init`. A unit test guarantees that this
/// parses to exactly [`Config::default`].
pub const CONFIG_TEMPLATE: &str = r#"# NestBruno configuration.
# Every key is optional; the values below are the defaults.

# Directory containing the NestJS sources, relative to this file.
source = "src"

# Bruno collection directory, relative to this file.
bruno = "bruno"

# Prefix of every generated request URL. The default is a Bruno variable you
# define in a Bruno environment (e.g. baseUrl = http://localhost:3000).
base_url = "{{baseUrl}}"

# Glob patterns, matched against paths relative to `source`, selecting the
# files that may contain controllers.
include = ["**/*.controller.ts"]

# Glob patterns, matched against paths relative to `source`. A path that
# matches any exclude pattern is skipped even if it matches `include`.
exclude = [
    "**/dist/**",
    "**/build/**",
    "**/coverage/**",
    "**/.tmp/**",
    "**/.cache/**",
]
"#;

/// Project configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// NestJS source directory, relative to the project root.
    pub source: PathBuf,
    /// Bruno collection directory, relative to the project root.
    pub bruno: PathBuf,
    /// Prefix of generated request URLs (may contain Bruno `{{variables}}`).
    pub base_url: String,
    /// Glob patterns selecting controller candidate files.
    pub include: Vec<String>,
    /// Glob patterns removing files from the candidate set.
    pub exclude: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            source: PathBuf::from("src"),
            bruno: PathBuf::from("bruno"),
            base_url: "{{baseUrl}}".to_owned(),
            include: vec!["**/*.controller.ts".to_owned()],
            exclude: ["dist", "build", "coverage", ".tmp", ".cache"]
                .iter()
                .map(|dir| format!("**/{dir}/**"))
                .collect(),
        }
    }
}

impl Config {
    /// Load `nestbruno.toml` from `root`.
    ///
    /// Returns `Ok(None)` when the file does not exist, so the caller can fall
    /// back to defaults. Any other problem is an error.
    pub fn load(root: &Path) -> Result<Option<(PathBuf, Self)>, Error> {
        let path = root.join(CONFIG_FILE_NAME);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(Error::Io { path, source }),
        };

        // Read one byte past the limit so an oversized file is detected
        // without reading it entirely.
        let mut text = String::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|source| Error::Io {
                path: path.clone(),
                source,
            })?;
        if text.len() as u64 > MAX_CONFIG_BYTES {
            return Err(Error::ConfigInvalid {
                path,
                message: format!("file is larger than {MAX_CONFIG_BYTES} bytes"),
            });
        }

        let config = Self::parse(&text, &path)?;
        Ok(Some((path, config)))
    }

    /// Parse and validate configuration text. `path` is used in errors only.
    pub fn parse(text: &str, path: &Path) -> Result<Self, Error> {
        let config: Self = toml::from_str(text).map_err(|source| Error::ConfigParse {
            path: path.to_owned(),
            source,
        })?;
        config.validate(path)?;
        Ok(config)
    }

    /// Reject values that could make the tool read or write outside the
    /// project, or that can never select anything.
    fn validate(&self, path: &Path) -> Result<(), Error> {
        let invalid = |message: String| Error::ConfigInvalid {
            path: path.to_owned(),
            message,
        };

        check_relative("source", &self.source).map_err(invalid)?;
        check_relative("bruno", &self.bruno).map_err(invalid)?;
        // The collection directory is written to in later phases; it must be a
        // real subdirectory, never the project root itself.
        if !self
            .bruno
            .components()
            .any(|c| matches!(c, Component::Normal(_)))
        {
            return Err(invalid(
                "`bruno` must name a subdirectory, not the project root".to_owned(),
            ));
        }
        // The URL prefix is written verbatim into a line-oriented file.
        if self.base_url.chars().any(char::is_control) || self.base_url.starts_with("'''") {
            return Err(invalid(
                "`base_url` must be a single line without control characters and must not start with '''"
                    .to_owned(),
            ));
        }
        if self.include.is_empty() {
            return Err(invalid(
                "`include` must contain at least one pattern, otherwise nothing can match"
                    .to_owned(),
            ));
        }
        Ok(())
    }
}

/// A configured directory must be a non-empty relative path without `..`.
fn check_relative(key: &str, value: &Path) -> Result<(), String> {
    if value.as_os_str().is_empty() {
        return Err(format!("`{key}` must not be empty"));
    }
    for component in value.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "`{key}` = {} must not contain `..`; paths must stay inside the project",
                    value.display()
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "`{key}` = {} must be relative to the project root, not absolute",
                    value.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, Error> {
        Config::parse(text, Path::new("nestbruno.toml"))
    }

    #[test]
    fn empty_file_yields_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(parse(CONFIG_TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let config = parse("source = \"apps/api/src\"").unwrap();
        assert_eq!(config.source, PathBuf::from("apps/api/src"));
        assert_eq!(config.bruno, Config::default().bruno);
        assert_eq!(config.include, Config::default().include);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = parse("sorce = \"src\"").unwrap_err();
        assert!(matches!(err, Error::ConfigParse { .. }));
        assert!(err.to_string().contains("sorce"));
    }

    #[test]
    fn wrong_types_are_rejected() {
        assert!(matches!(
            parse("include = \"**/*.ts\"").unwrap_err(),
            Error::ConfigParse { .. }
        ));
    }

    #[test]
    fn parent_directory_components_are_rejected() {
        for text in ["source = \"../src\"", "bruno = \"out/../../x\""] {
            let err = parse(text).unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{text}");
            assert!(err.to_string().contains(".."), "{text}");
        }
    }

    #[test]
    fn absolute_paths_are_rejected() {
        let abs = if cfg!(windows) { "C:\\\\x" } else { "/etc" };
        for key in ["source", "bruno"] {
            let err = parse(&format!("{key} = \"{abs}\"")).unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{key}");
        }
    }

    #[test]
    fn empty_directories_are_rejected() {
        assert!(matches!(
            parse("source = \"\"").unwrap_err(),
            Error::ConfigInvalid { .. }
        ));
    }

    #[test]
    fn bruno_must_not_be_the_project_root() {
        for value in [".", "./"] {
            let err = parse(&format!("bruno = \"{value}\"")).unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{value}");
        }
    }

    #[test]
    fn source_may_be_the_project_root() {
        assert_eq!(parse("source = \".\"").unwrap().source, PathBuf::from("."));
    }

    #[test]
    fn base_url_defaults_to_a_bruno_variable_and_can_be_overridden() {
        assert_eq!(Config::default().base_url, "{{baseUrl}}");
        let config = parse("base_url = \"http://localhost:3000\"").unwrap();
        assert_eq!(config.base_url, "http://localhost:3000");
    }

    #[test]
    fn base_url_must_be_a_single_safe_line() {
        for value in ["a\\nb", "a\\rb", "'''x", "a\\u0000b"] {
            let err = parse(&format!("base_url = \"{value}\"")).unwrap_err();
            assert!(matches!(err, Error::ConfigInvalid { .. }), "{value}");
        }
    }

    #[test]
    fn empty_include_is_rejected() {
        assert!(matches!(
            parse("include = []").unwrap_err(),
            Error::ConfigInvalid { .. }
        ));
    }

    #[test]
    fn load_returns_none_when_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Config::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn load_reads_and_validates_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), "bruno = \"api-docs\"").unwrap();
        let (path, config) = Config::load(dir.path()).unwrap().unwrap();
        assert_eq!(path, dir.path().join(CONFIG_FILE_NAME));
        assert_eq!(config.bruno, PathBuf::from("api-docs"));
    }

    #[test]
    fn load_rejects_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let big = format!("# {}\n", "x".repeat(MAX_CONFIG_BYTES as usize));
        std::fs::write(dir.path().join(CONFIG_FILE_NAME), big).unwrap();
        let err = Config::load(dir.path()).unwrap_err();
        assert!(matches!(err, Error::ConfigInvalid { .. }));
    }

    #[test]
    fn load_reports_a_directory_named_like_the_config_as_io_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(CONFIG_FILE_NAME)).unwrap();
        assert!(matches!(
            Config::load(dir.path()).unwrap_err(),
            Error::Io { .. }
        ));
    }
}
