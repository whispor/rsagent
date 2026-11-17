//! Conformance tests every built-in tool must pass.
//!
//! These encode the `Tool` invariants from `docs/EXTENSIBILITY.md` §2.4, so a new
//! built-in that breaks the contract fails here instead of in production. Adding a
//! tool to [`builtin_tools`] automatically opts it into every check below.

use std::collections::HashSet;

use rsagent_agent::builtin_tools;
use rsagent_ai::ToolSpec;
use serde_json::Value;

/// `snake_case`: lowercase ASCII letters, digits, single underscores, no edges.
fn is_snake_case(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('_')
        && !name.ends_with('_')
        && !name.contains("__")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn specs() -> Vec<ToolSpec> {
    builtin_tools().iter().map(|tool| tool.spec()).collect()
}

#[test]
fn names_are_unique_and_snake_case() {
    // §2.4 rule 5 — the model calls tools by name, so names must be unambiguous.
    let mut seen = HashSet::new();
    for spec in specs() {
        assert!(
            is_snake_case(&spec.name),
            "tool name `{}` is not snake_case",
            spec.name
        );
        assert!(
            seen.insert(spec.name.clone()),
            "duplicate tool name `{}`",
            spec.name
        );
    }
}

#[test]
fn every_tool_describes_itself() {
    // The description is the model's only hint about when to use the tool.
    for tool in builtin_tools() {
        let spec = tool.spec();
        assert!(
            !spec.description.trim().is_empty(),
            "tool `{}` has an empty description",
            spec.name
        );
        assert!(
            !tool.label().trim().is_empty(),
            "tool `{}` has an empty label",
            spec.name
        );
    }
}

#[test]
fn parameters_are_an_object_schema() {
    // §2.4 rule 6 — the model's function-calling contract is an object schema.
    for spec in specs() {
        assert_eq!(
            spec.parameters.get("type").and_then(Value::as_str),
            Some("object"),
            "tool `{}` parameters must be an object schema",
            spec.name
        );

        let Some(properties) = spec.parameters.get("properties").and_then(Value::as_object) else {
            panic!("tool `{}` schema needs an object `properties`", spec.name);
        };

        let Some(required) = spec.parameters.get("required").and_then(Value::as_array) else {
            continue;
        };
        for key in required {
            let key = key.as_str().expect("`required` entries are strings");
            assert!(
                properties.contains_key(key),
                "tool `{}` requires undeclared property `{key}`",
                spec.name
            );
        }
    }
}

#[test]
fn state_mutating_tools_require_approval() {
    // §2.4 rule 4 — anything that writes, executes, or reaches the network must ask.
    let must_ask = ["write_file", "edit_file", "webfetch", "bash"];
    for tool in builtin_tools() {
        let name = tool.spec().name;
        if must_ask.contains(&name.as_str()) {
            assert!(
                tool.requires_approval(),
                "`{name}` has side effects and must require approval"
            );
        }
    }
}

#[test]
fn read_only_tools_do_not_require_approval() {
    // The mirror of the rule above: read-only tools must stay frictionless.
    let read_only = ["read_file", "list_dir", "search", "glob"];
    for tool in builtin_tools() {
        let name = tool.spec().name;
        if read_only.contains(&name.as_str()) {
            assert!(
                !tool.requires_approval(),
                "`{name}` is read-only and should not require approval"
            );
        }
    }
}

#[test]
fn filesystem_writers_and_shell_run_sequentially() {
    // Tools that mutate the same working tree must not race each other.
    let must_be_sequential = ["write_file", "edit_file", "bash"];
    for tool in builtin_tools() {
        let name = tool.spec().name;
        if must_be_sequential.contains(&name.as_str()) {
            assert!(
                tool.is_sequential(),
                "`{name}` mutates state and must be sequential"
            );
        }
    }
}

#[test]
fn the_builtin_set_matches_what_the_docs_claim() {
    // A guard so `--list-tools`, the README, and reality cannot drift apart silently.
    let mut names: Vec<String> = specs().into_iter().map(|spec| spec.name).collect();
    names.sort();
    let expected: Vec<String> = [
        "bash",
        "edit_file",
        "glob",
        "list_dir",
        "read_file",
        "search",
        "webfetch",
        "write_file",
    ]
    .map(str::to_string)
    .to_vec();

    assert_eq!(names, expected);
}
