//! Project configuration: `rsagent.toml`.
//!
//! One file, found at or above the working directory. Precedence is
//! **CLI flags > environment > this file > built-in defaults**
//! (`docs/EXTENSIBILITY.md` §3.3). A missing file simply means "no configuration"; a file
//! that exists but does not parse is reported, never silently ignored.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The configuration file name.
pub(crate) const CONFIG_FILE: &str = "rsagent.toml";

/// The whole configuration file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// The `[tools]` table.
    #[serde(default)]
    pub tools: Tools,
}

/// The `[tools]` table.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tools {
    /// `[[tools.sources]]` entries, in the order they should be assembled.
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
}

/// One `[[tools.sources]]` entry.
///
/// A single flat struct covers every kind, so the file stays easy to read; fields that do
/// not apply to a given kind are simply unused. `deny_unknown_fields` turns a typo into an
/// error instead of a silently ignored line.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceConfig {
    /// The source kind: `builtin`, `local`, `mcp-stdio`, `mcp-http`, `skill`, or `web`
    /// (`docs/EXTENSIBILITY.md` §3.2).
    pub kind: String,
    /// A stable id, unique among sources.
    pub id: String,
    /// An optional prefix applied to this source's tool names.
    #[serde(default)]
    pub prefix: Option<String>,
    /// Keep only these tools (built-in sources).
    #[serde(default)]
    pub only: Option<Vec<String>>,
    /// Always drop these tools (built-in sources).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// The program to run (local and `mcp-stdio` sources).
    #[serde(default)]
    pub command: Option<String>,
    /// Fixed arguments passed to `command` before the phase flag (local and `mcp-stdio`
    /// sources).
    #[serde(default)]
    pub args: Vec<String>,
    /// The skill folder (`skill` sources): a directory holding `SKILL.md` and `skill.toml`.
    #[serde(default)]
    pub path: Option<String>,
    /// The endpoint URL (`mcp-http` and `web` sources).
    #[serde(default)]
    pub url: Option<String>,
    /// Extra request headers, e.g. `Authorization` (`mcp-http` and `web` sources).
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// The HTTP method (`web` sources; default `GET`).
    #[serde(default)]
    pub method: Option<String>,
    /// The tool name the model sees (`web` sources; default: the source id).
    #[serde(default)]
    pub name: Option<String>,
    /// A one-line description for the model (`web` sources).
    #[serde(default)]
    pub description: Option<String>,
    /// The JSON Schema the model must satisfy (`web` sources; default: an empty object).
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    /// Whether a call needs the user's approval first (`web` sources; default `true`).
    #[serde(default)]
    pub requires_approval: Option<bool>,
    /// Per-call timeout in seconds (local, MCP, and `web` sources; default 60).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl Config {
    /// Find and parse `rsagent.toml` at or above `cwd`.
    ///
    /// # Errors
    ///
    /// Returns a message when the file exists but cannot be read or parsed, so the caller
    /// can warn and fall back instead of pretending the file was never there.
    pub(crate) fn load(cwd: &Path) -> Result<(Self, Option<PathBuf>), String> {
        let Some(path) = find_upward(cwd, CONFIG_FILE) else {
            return Ok((Config::default(), None));
        };
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let config =
            Config::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok((config, Some(path)))
    }

    /// Parse configuration text.
    ///
    /// # Errors
    ///
    /// Returns a message when the text is not valid TOML or has an unknown field.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|error| error.to_string())
    }
}

/// The nearest `name` at or above `start`, or `None` when there is none.
///
/// Walking upward lets a repo-root file apply from any subdirectory. Shared with the
/// `AGENTS.md` lookup, which has the same rule.
pub(crate) fn find_upward(start: &Path, name: &str) -> Option<PathBuf> {
    let start = std::fs::canonicalize(start).ok()?;
    start
        .ancestors()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn a_full_file_parses() {
        let config = Config::parse(
            r#"
            [[tools.sources]]
            kind = "builtin"
            id = "core"
            exclude = ["bash"]

            [[tools.sources]]
            kind = "local"
            id = "release"
            command = "./scripts/release-tool"
            args = ["--verbose"]
            timeout_secs = 5
            prefix = "rel"
        "#,
        )
        .expect("parse");

        assert_eq!(config.tools.sources.len(), 2);
        let builtin = &config.tools.sources[0];
        assert_eq!(builtin.kind, "builtin");
        assert_eq!(builtin.exclude, vec!["bash".to_string()]);
        let local = &config.tools.sources[1];
        assert_eq!(local.command.as_deref(), Some("./scripts/release-tool"));
        assert_eq!(local.args, vec!["--verbose".to_string()]);
        assert_eq!(local.timeout_secs, Some(5));
        assert_eq!(local.prefix.as_deref(), Some("rel"));
    }

    #[test]
    fn an_mcp_http_source_parses_its_url_and_headers() {
        let config = Config::parse(
            r#"
            [[tools.sources]]
            kind = "mcp-http"
            id = "remote"
            url = "https://mcp.example.com/mcp"
            prefix = "rm"
            timeout_secs = 30
            headers = { Authorization = "Bearer token" }
        "#,
        )
        .expect("parse");

        let source = &config.tools.sources[0];
        assert_eq!(source.kind, "mcp-http");
        assert_eq!(source.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert_eq!(source.timeout_secs, Some(30));
        let headers = source.headers.as_ref().expect("headers");
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer token")
        );
    }

    #[test]
    fn a_skill_source_parses_its_path() {
        let config = Config::parse(
            r#"
            [[tools.sources]]
            kind = "skill"
            id = "team"
            path = "./skills/team"
            prefix = "team"
        "#,
        )
        .expect("parse");

        let source = &config.tools.sources[0];
        assert_eq!(source.kind, "skill");
        assert_eq!(source.path.as_deref(), Some("./skills/team"));
        assert_eq!(source.prefix.as_deref(), Some("team"));
    }

    #[test]
    fn a_web_source_parses_its_endpoint_and_tool() {
        let config = Config::parse(
            r#"
            [[tools.sources]]
            kind = "web"
            id = "weather"
            url = "https://api.example.com/weather"
            method = "POST"
            name = "get_weather"
            description = "Look up the weather."
            requires_approval = false
            headers = { Authorization = "Bearer token" }
            [tools.sources.parameters]
            type = "object"
            properties.city = { type = "string" }
            required = ["city"]
        "#,
        )
        .expect("parse");

        let source = &config.tools.sources[0];
        assert_eq!(source.kind, "web");
        assert_eq!(
            source.url.as_deref(),
            Some("https://api.example.com/weather")
        );
        assert_eq!(source.method.as_deref(), Some("POST"));
        assert_eq!(source.name.as_deref(), Some("get_weather"));
        assert_eq!(source.description.as_deref(), Some("Look up the weather."));
        assert_eq!(source.requires_approval, Some(false));
        assert_eq!(
            source.parameters.as_ref().expect("parameters")["required"],
            serde_json::json!(["city"])
        );
    }

    #[test]
    fn an_empty_file_is_an_empty_config() {
        let config = Config::parse("").expect("parse");
        assert!(config.tools.sources.is_empty());
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let error = Config::parse(
            r#"
            [[tools.sources]]
            kind = "builtin"
            id = "core"
            presfix = "typo"
        "#,
        )
        .expect_err("a typo must be reported");
        assert!(error.contains("presfix"), "{error}");
    }

    #[test]
    fn a_source_without_a_kind_is_refused() {
        assert!(Config::parse("[[tools.sources]]\nid = \"core\"\n").is_err());
    }

    #[test]
    fn a_source_without_an_id_is_refused() {
        assert!(Config::parse("[[tools.sources]]\nkind = \"builtin\"\n").is_err());
    }
}
