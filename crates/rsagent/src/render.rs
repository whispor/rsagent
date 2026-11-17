//! Terminal styling shared by the CLI's output paths.

use std::io::IsTerminal;

/// The user's color preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorChoice {
    /// Colorize when stdout is a terminal and `NO_COLOR` is unset.
    Auto,
    /// Never colorize.
    Never,
}

/// ANSI styling that degrades to plain text.
///
/// Color is disabled by `--no-color`, by a non-empty `NO_COLOR` environment variable,
/// or when stdout is not a terminal, so redirected output and log files stay clean.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Palette {
    enabled: bool,
}

impl Palette {
    /// Decide whether to colorize output.
    pub(crate) fn detect(choice: ColorChoice) -> Self {
        let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
        let want_color = choice == ColorChoice::Auto && !no_color_env;
        Palette {
            enabled: want_color && std::io::stdout().is_terminal(),
        }
    }

    /// Wrap `text` in the SGR `code`, or return it unchanged when color is disabled.
    pub(crate) fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// Dim (faint) text, used for secondary status lines.
    pub(crate) fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    /// Yellow text, used for prompts and warnings.
    pub(crate) fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    /// Red text, used for errors.
    pub(crate) fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    /// Green text, used for successful tool results.
    pub(crate) fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    /// Cyan text, used for tool-call headers.
    pub(crate) fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }
}
