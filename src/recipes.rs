//! Recipes: named prompts that open a new agent in a split and kick it off,
//! picked from the palette (⌘R, or `r` in the git panel).
//!
//! ```lua
//! require("wezzy").setup({
//!   recipes = {
//!     { name = "Review changes", harness = "claude", prompt = "/code-review" },
//!     { name = "Tests for file", prompt = "Write tests for {file}" },
//!   },
//! })
//! ```
//!
//! `harness` defaults to the default harness. Prompt variables:
//!   {cwd} {branch}      where the agent starts
//!   {diff} {staged}     `git diff HEAD` / `git diff --cached`
//!   {file}              the file under the git panel cursor
//!   {last_output}       the last command + output in the active pane
//!
//! A recipe with the same name as a built-in replaces it.

use crate::app::TermApp;
use crate::settings::{self, Harness};
use gpui::{Context, Window};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Prompts longer than this go through a file the agent is told to read,
/// so huge diffs don't hit argv / shell limits.
const MAX_INLINE_PROMPT: usize = 6000;

#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub name: String,
    /// harness name; `None` = the default harness
    pub harness: Option<String>,
    pub prompt: String,
}

pub fn builtins() -> Vec<Recipe> {
    let r = |name: &str, harness: Option<&str>, prompt: &str| Recipe {
        name: name.into(),
        harness: harness.map(Into::into),
        prompt: prompt.into(),
    };
    vec![
        r("Review changes", Some("claude"), "/code-review"),
        r("Simplify changes", Some("claude"), "/simplify"),
        r(
            "Explain changes",
            None,
            "Explain the uncommitted changes in this repo (run `git diff HEAD`): what changed, \
             why it likely changed, and anything that looks risky. Don't edit files.",
        ),
        r(
            "Fix last error",
            None,
            "This failed. Find the cause and fix it:\n\n{last_output}",
        ),
    ]
}

/// User recipes first (in config order), then built-ins they don't replace.
pub fn all() -> Vec<Recipe> {
    let mut out = settings::get().recipes;
    for b in builtins() {
        if !out.iter().any(|r| r.name == b.name) {
            out.push(b);
        }
    }
    out
}

/// What the variables expand to; gathered lazily, only for variables the prompt uses.
pub struct Vars {
    pub cwd: Option<PathBuf>,
    pub file: Option<String>,
    pub last_output: Option<String>,
}

fn git(dir: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
        .unwrap_or_default()
}

/// Fill in `{var}`s. Unknown names are left as written.
pub fn expand(prompt: &str, vars: &Vars) -> Result<String, String> {
    let cwd = vars.cwd.clone().unwrap_or_else(|| config::HOME_DIR.clone());
    let mut out = String::new();
    let mut rest = prompt;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let name = &after[..end];
        let value = match name {
            "cwd" => cwd.display().to_string(),
            "branch" => git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]),
            "diff" => non_empty(git(&cwd, &["diff", "HEAD", "--no-color"]), "No uncommitted changes")?,
            "staged" => non_empty(git(&cwd, &["diff", "--cached", "--no-color"]), "Nothing staged")?,
            "file" => vars
                .file
                .clone()
                .ok_or("Put the git panel cursor on a file first")?,
            "last_output" => vars
                .last_output
                .clone()
                .filter(|s| !s.trim().is_empty())
                .ok_or("No output to send")?,
            _ => format!("{{{name}}}"),
        };
        out.push_str(&value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn non_empty(s: String, err: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err(err.into())
    } else {
        Ok(s)
    }
}

/// How each harness takes an initial interactive prompt.
fn prompt_args(harness: &str, prompt: String) -> Vec<String> {
    match harness {
        "gemini" => vec!["-i".into(), prompt],
        "opencode" => vec!["--prompt".into(), prompt],
        // aider's --message would run once and exit; open it interactive instead
        "aider" => vec![],
        _ => vec![prompt],
    }
}

/// Long prompts go into a file; the agent gets a short instruction to read it.
fn deliverable(prompt: String) -> String {
    deliverable_in(&std::env::temp_dir().join("wezzy-recipes"), prompt)
}

fn deliverable_in(dir: &Path, prompt: String) -> String {
    if prompt.len() <= MAX_INLINE_PROMPT {
        return prompt;
    }
    std::fs::create_dir_all(dir).ok();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("prompt-{stamp}.md"));
    match std::fs::write(&path, &prompt) {
        Ok(()) => format!(
            "Your task is in {} — read it and follow the instructions.",
            path.display()
        ),
        Err(_) => prompt,
    }
}

impl TermApp {
    /// Expand a recipe and open its agent; ⌘↵ in the palette opens a tab instead.
    pub(crate) fn run_recipe(
        &mut self,
        name: &str,
        placement: crate::launch::Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(recipe) = all().into_iter().find(|r| r.name == name) else {
            return;
        };
        let s = settings::get();
        let harness_name = recipe.harness.clone().unwrap_or_else(|| s.default_harness_name());
        let Some(harness) = s.harness(&harness_name) else {
            self.flash(format!("{}: no harness named \"{harness_name}\"", recipe.name));
            return;
        };

        let pane = self.active_pane();
        let cwd = pane
            .as_ref()
            .and_then(|p| p.get_current_working_dir(mux::pane::CachePolicy::AllowStale))
            .map(|u| PathBuf::from(u.path()));
        let vars = Vars {
            cwd,
            file: self.git_cursor_file(),
            last_output: recipe
                .prompt
                .contains("{last_output}")
                .then(|| pane.as_ref().map(crate::sendout::capture))
                .flatten()
                .map(|c| match c.command {
                    Some(cmd) => format!("$ {cmd}\n{}", c.output),
                    None => c.output,
                }),
        };
        let prompt = match expand(&recipe.prompt, &vars) {
            Ok(p) => deliverable(p),
            Err(e) => {
                self.flash(format!("{}: {e}", recipe.name));
                return;
            }
        };
        let mut argv = harness.argv.clone();
        argv.extend(prompt_args(&harness.name, prompt));
        let launch = Harness {
            name: harness.name.clone(),
            argv,
        };
        self.region = crate::regions::Region::Terminal;
        self.spawn_harness(&launch, placement, window, cx);
    }

    /// Path of the file under the git panel cursor, if the panel is open.
    fn git_cursor_file(&self) -> Option<String> {
        use crate::gitpanel::Row;
        if !self.diff_open {
            return None;
        }
        let snap = crate::diff::snapshot()?;
        let rows = self.git.rows(&snap);
        let (sec, fi) = match rows.get(self.git.cursor)? {
            Row::File(s, f) | Row::Hunk(s, f, _) | Row::Line(s, f, ..) => (*s, *f),
            Row::Section(_) | Row::Feedback | Row::Comment(_) | Row::Draft => return None,
        };
        Some(snap.files(sec).get(fi)?.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        Vars {
            cwd: Some("/tmp".into()),
            file: Some("src/a.rs".into()),
            last_output: None,
        }
    }

    #[test]
    fn expands_known_and_keeps_unknown() {
        assert_eq!(
            expand("Test {file} in {cwd}; keep {other} and {", &vars()).unwrap(),
            "Test src/a.rs in /tmp; keep {other} and {"
        );
    }

    #[test]
    fn missing_values_are_errors() {
        assert!(expand("Fix {last_output}", &vars()).is_err());
        let mut v = vars();
        v.file = None;
        assert!(expand("{file}", &v).is_err());
    }

    #[test]
    fn long_prompts_go_to_a_file() {
        let long = "x".repeat(MAX_INLINE_PROMPT + 1);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-recipes");
        let msg = deliverable_in(&dir, long.clone());
        let path = msg.split_whitespace().nth(4).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), long);
        let _ = std::fs::remove_dir_all(dir);
        assert_eq!(deliverable_in(Path::new("/nonexistent"), "short".into()), "short");
    }
}
