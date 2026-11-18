//! A source backed by a *skill*: a folder of scripts, declared by a manifest.
//!
//! A skill is a directory that holds
//!
//! - `SKILL.md` — the skill's documentation, and the marker that the folder *is* a skill;
//! - `skill.toml` — the tool manifest: one `[[tool]]` entry per tool, naming the script to
//!   run and the parameters it takes.
//!
//! Each tool runs its `command` as a subprocess with the tool's arguments written to
//! stdin, and reads a `{"content": ...}` or `{"error": ...}` reply — the same reply
//! contract as the [`local`](super::local) source, so a script is written the same way
//! whichever source runs it. The difference is where the tool list comes from: `local`
//! asks the program (`--describe`), while a skill reads its manifest, so the folder is
//! self-describing without being run.
//!
//! # The manifest
//!
//! ```toml
//! # skill.toml
//! [[tool]]
//! name = "lint"
//! description = "Lint a path with the team's rules."
//! command = "scripts/lint.sh"        # relative to the skill folder, or absolute
//! args = ["--strict"]                # fixed args, before the tool's own input
//! requires_approval = false          # default: true
//! [tool.parameters]                  # JSON Schema for the model; default is an empty object
//! type = "object"
//! properties.path = { type = "string" }
//! required = ["path"]
//! ```
//!
//! # Failure classes
//!
//! The same split as `local`: a run that cannot start, times out, exits non-zero, or
//! prints non-JSON is **connection-class** and trips the breaker; a well-formed
//! `{"error": ...}` reply is a **business** failure and does not.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rsagent_ai::ToolSpec;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::breaker::{BreakerPolicy, BreakerReport, BreakerState, CircuitBreaker};
use crate::source::{SourceHealth, SourceKind, ToolSource};
use crate::tool::{Tool, ToolContext, ToolResult};

use super::subprocess::{parse_response, run};

/// The documentation file that marks a folder as a skill.
const SKILL_FILE: &str = "SKILL.md";

/// The tool manifest.
const MANIFEST_FILE: &str = "skill.toml";

/// Per-call timeout when the configuration does not set one.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The `skill.toml` manifest.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// The tools this skill offers.
    #[serde(default, rename = "tool")]
    tools: Vec<ToolConfig>,
}

/// One `[[tool]]` entry.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolConfig {
    /// The tool name as the model sees it (before any source prefix is applied).
    name: String,
    /// A one-line description for the model.
    #[serde(default)]
    description: String,
    /// The script to run, relative to the skill folder (or absolute).
    command: String,
    /// Fixed arguments passed before the tool's own input.
    #[serde(default)]
    args: Vec<String>,
    /// The JSON Schema the model must satisfy; defaults to an empty object.
    #[serde(default)]
    parameters: Option<Value>,
    /// Whether a call needs the user's approval first. Defaults to `true`.
    #[serde(default = "approval_default")]
    requires_approval: bool,
}

/// Approval is opt-out: an arbitrary script may do anything.
fn approval_default() -> bool {
    true
}

/// A source backed by a skill folder.
///
/// Construct one with [`new`](SkillSource::new), which reads the manifest and resolves
/// every tool's script. Loading is synchronous — nothing is spawned until a tool is called.
#[derive(Debug)]
pub struct SkillSource {
    id: String,
    timeout: Duration,
    tools: Vec<SkillToolSpec>,
    breaker: Arc<CircuitBreaker>,
}

impl SkillSource {
    /// Read the skill at `path`.
    ///
    /// # Errors
    ///
    /// Returns a message when `path` is not a skill (no `SKILL.md`), when `skill.toml` is
    /// missing or malformed, or when a tool names a `command` that does not exist.
    pub fn new(
        id: impl Into<String>,
        path: impl AsRef<Path>,
        timeout: Option<Duration>,
    ) -> Result<Self, String> {
        let dir = path.as_ref();
        if !dir.join(SKILL_FILE).is_file() {
            return Err(format!(
                "`{}` is not a skill: it has no {}",
                dir.display(),
                SKILL_FILE
            ));
        }

        let manifest_path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
        let manifest: Manifest = toml::from_str(&text)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;

        let tools = manifest
            .tools
            .into_iter()
            .map(|config| resolve_tool(dir, config))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SkillSource {
            id: id.into(),
            timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
            tools,
            breaker: Arc::new(CircuitBreaker::new(BreakerPolicy::default())),
        })
    }

    /// Replace the default breaker policy (used by tests and by a future `doctor`).
    #[must_use]
    pub fn with_breaker_policy(mut self, policy: BreakerPolicy) -> Self {
        self.breaker = Arc::new(CircuitBreaker::new(policy));
        self
    }

    /// The current health, derived from the circuit breaker.
    fn health(&self) -> SourceHealth {
        let now = Instant::now();
        match self.breaker.state(now) {
            BreakerState::Closed => SourceHealth::Healthy,
            BreakerState::HalfOpen => {
                SourceHealth::Unhealthy("circuit half-open; the next call probes it".to_string())
            }
            BreakerState::Open => {
                let wait = self.breaker.retry_after(now).map_or(0, |d| d.as_secs());
                SourceHealth::Unhealthy(format!("circuit open; retry in {wait}s"))
            }
        }
    }
}

/// Turn one manifest entry into a runnable tool, resolving and checking its script.
fn resolve_tool(dir: &Path, config: ToolConfig) -> Result<SkillToolSpec, String> {
    let command = Path::new(&config.command);
    let program = if command.is_absolute() {
        command.to_path_buf()
    } else {
        dir.join(command)
    };
    if !program.is_file() {
        return Err(format!(
            "tool `{}` runs `{}`, which does not exist",
            config.name,
            program.display()
        ));
    }

    let mut argv = vec![program.to_string_lossy().into_owned()];
    argv.extend(config.args);

    Ok(SkillToolSpec {
        name: config.name,
        description: config.description,
        parameters: config
            .parameters
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
        requires_approval: config.requires_approval,
        argv,
    })
}

impl ToolSource for SkillSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Skill
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        self.tools
            .iter()
            .map(|spec| -> Box<dyn Tool> {
                Box::new(SkillTool {
                    source_id: self.id.clone(),
                    timeout: self.timeout,
                    spec: spec.clone(),
                    breaker: Arc::clone(&self.breaker),
                })
            })
            .collect()
    }

    fn health(&self) -> SourceHealth {
        self.health()
    }

    fn breaker_report(&self) -> Option<BreakerReport> {
        Some(self.breaker.report())
    }

    fn restore_breaker(&self, report: &BreakerReport) {
        self.breaker.restore(report);
    }
}

/// One tool declared by the manifest, with its command line already resolved.
#[derive(Debug, Clone)]
struct SkillToolSpec {
    name: String,
    description: String,
    parameters: Value,
    requires_approval: bool,
    /// The full command line: the resolved script, then the fixed `args`.
    argv: Vec<String>,
}

/// A tool offered by a [`SkillSource`].
///
/// Each call is a fresh process, so — unlike `local`'s single shared program — these tools
/// are *not* sequential: the agent may run several at once.
#[derive(Debug)]
struct SkillTool {
    source_id: String,
    timeout: Duration,
    spec: SkillToolSpec,
    breaker: Arc<CircuitBreaker>,
}

#[async_trait]
impl Tool for SkillTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            self.spec.name.clone(),
            self.spec.description.clone(),
            self.spec.parameters.clone(),
        )
    }

    fn requires_approval(&self) -> bool {
        self.spec.requires_approval
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, String> {
        let now = Instant::now();
        if !self.breaker.allows(now) {
            let wait = self.breaker.retry_after(now).map_or(0, |d| d.as_secs());
            return Err(format!(
                "tool source `{}` is unavailable (circuit open); retry in {wait}s",
                self.source_id
            ));
        }

        // The script receives the tool's arguments directly, as one JSON object on stdin.
        let stdout = match run(&self.spec.argv, Some(&args), self.timeout).await {
            Ok(stdout) => stdout,
            Err(error) => {
                self.breaker.record_failure(Instant::now());
                return Err(error);
            }
        };

        let value: Value = match serde_json::from_str(&stdout) {
            Ok(value) => value,
            Err(error) => {
                self.breaker.record_failure(Instant::now());
                return Err(format!(
                    "`{}` printed invalid JSON: {error}",
                    self.spec.name
                ));
            }
        };

        // The script answered: it is reachable, whatever the answer says.
        self.breaker.record_success();
        parse_response(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::{SkillSource, SKILL_FILE};
    use crate::source::{SourceHealth, SourceKind, ToolSource};
    use std::path::PathBuf;

    /// A fresh, empty directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rsagent-skill-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch dir");
        dir
    }

    /// Write a skill folder: `SKILL.md`, `skill.toml`, and a dummy `command` script.
    fn write_skill(name: &str, manifest: &str) -> PathBuf {
        let dir = scratch(name);
        std::fs::write(dir.join(SKILL_FILE), "# A test skill\n").expect("write SKILL.md");
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").expect("write the script");
        std::fs::write(dir.join("skill.toml"), manifest).expect("write skill.toml");
        dir
    }

    #[test]
    fn a_skill_loads_its_tools() {
        let dir = write_skill(
            "loads",
            r#"
            [[tool]]
            name = "lint"
            description = "Lint a path."
            command = "run.sh"
            "#,
        );
        let source = SkillSource::new("team", &dir, None).expect("load");

        assert_eq!(source.kind(), SourceKind::Skill);
        assert_eq!(source.id(), "team");
        assert_eq!(source.health(), SourceHealth::Healthy);

        let tools = source.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].spec().name, "lint");
        assert_eq!(tools[0].spec().description, "Lint a path.");
        assert!(tools[0].requires_approval(), "approval is opt-out");
        assert!(!tools[0].is_sequential(), "each call is a fresh process");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_relative_command_resolves_against_the_skill_folder() {
        let dir = write_skill("relative", "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\n");
        let source = SkillSource::new("s", &dir, None).expect("load");
        let argv = &source.tools[0].argv;
        assert_eq!(argv.len(), 1);
        assert_eq!(PathBuf::from(&argv[0]), dir.join("run.sh"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fixed_args_follow_the_script() {
        let dir = write_skill(
            "args",
            "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\nargs = [\"--strict\", \"-x\"]\n",
        );
        let source = SkillSource::new("s", &dir, None).expect("load");
        let argv = &source.tools[0].argv;
        assert_eq!(
            &argv[1..],
            &["--strict".to_string(), "-x".to_string()],
            "{argv:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parameters_default_to_an_empty_object() {
        let dir = write_skill(
            "params-default",
            "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\n",
        );
        let source = SkillSource::new("s", &dir, None).expect("load");
        assert_eq!(
            source.tools[0].parameters,
            serde_json::json!({ "type": "object", "properties": {} })
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_schema_is_passed_through() {
        let dir = write_skill(
            "params-explicit",
            r#"
            [[tool]]
            name = "t"
            command = "run.sh"
            [tool.parameters]
            type = "object"
            properties.path = { type = "string" }
            required = ["path"]
            "#,
        );
        let source = SkillSource::new("s", &dir, None).expect("load");
        assert_eq!(
            source.tools[0].parameters,
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            })
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_approval_flag_is_honoured() {
        let dir = write_skill(
            "approval",
            "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\nrequires_approval = false\n",
        );
        let source = SkillSource::new("s", &dir, None).expect("load");
        assert!(!source.tools()[0].requires_approval());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_without_skill_md_is_refused() {
        let dir = scratch("no-skill-md");
        std::fs::write(
            dir.join("skill.toml"),
            "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\n",
        )
        .expect("write");
        let error = SkillSource::new("s", &dir, None).expect_err("a skill needs SKILL.md");
        assert!(error.contains(SKILL_FILE), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_manifest_is_refused() {
        let dir = scratch("no-manifest");
        std::fs::write(dir.join(SKILL_FILE), "# docs\n").expect("write");
        let error = SkillSource::new("s", &dir, None).expect_err("a skill needs a manifest");
        assert!(error.contains("skill.toml"), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_command_is_refused() {
        let dir = write_skill(
            "no-command",
            "[[tool]]\nname = \"t\"\ncommand = \"does-not-exist.sh\"\n",
        );
        let error = SkillSource::new("s", &dir, None).expect_err("the script must exist");
        assert!(error.contains("does not exist"), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_manifest_field_is_refused() {
        let dir = write_skill(
            "unknown",
            "[[tool]]\nname = \"t\"\ncommand = \"run.sh\"\ncommnd = \"typo\"\n",
        );
        let error = SkillSource::new("s", &dir, None).expect_err("a typo must be reported");
        assert!(error.contains("commnd"), "{error}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_tool_without_a_command_is_refused() {
        let dir = write_skill("no-command-field", "[[tool]]\nname = \"t\"\n");
        assert!(SkillSource::new("s", &dir, None).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
