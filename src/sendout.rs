//! "Send output to agent": paste the last command + its output from the active
//! pane into an agent's prompt (without submitting), so you can add a note.
//!
//! The command/output boundary comes from shell-integration markers (OSC 133),
//! which WezTerm records per cell and which survive the trip from the session
//! server. Without them we fall back to the visible screen.

use crate::app::TermApp;
use crate::palette::Scope;
use gpui::{Context, Window};
use mux::pane::{CachePolicy, Pane, PaneId};
use mux::Mux;
use std::sync::Arc;
use wezterm_term::{SemanticType, StableRowIndex};

const SCAN_LINES: isize = 3000;
const MAX_OUTPUT_LINES: usize = 400;

pub struct Captured {
    pub command: Option<String>,
    pub output: String,
    /// true when markers delimited it; false = visible screen fallback
    pub exact: bool,
    pub truncated: bool,
}

fn line_text(line: &wezterm_term::Line, want: impl Fn(SemanticType) -> bool) -> String {
    let mut s = String::new();
    for cell in line.visible_cells() {
        if want(cell.attrs().semantic_type()) {
            s.push_str(cell.str());
        }
    }
    s.trim_end().to_string()
}

pub fn capture(pane: &Arc<dyn Pane>) -> Captured {
    let dims = pane.get_dimensions();
    let end = dims.physical_top + dims.viewport_rows as StableRowIndex;
    let start = (end - SCAN_LINES as StableRowIndex).max(dims.scrollback_top);
    let (_, lines) = pane.get_lines(start..end);

    let has = |l: &wezterm_term::Line, t: SemanticType| {
        l.visible_cells().any(|c| c.attrs().semantic_type() == t)
    };
    let marked = lines
        .iter()
        .any(|l| has(l, SemanticType::Prompt) || has(l, SemanticType::Input));

    if !marked {
        let top = dims.physical_top;
        let (_, screen) = pane.get_lines(top..end);
        let mut text: Vec<String> = screen
            .iter()
            .map(|l| line_text(l, |_| true))
            .collect();
        while text.last().is_some_and(|l| l.is_empty()) {
            text.pop();
        }
        return Captured {
            command: None,
            output: text.join("\n"),
            exact: false,
            truncated: false,
        };
    }

    // Walk up from the bottom: skip the current (empty) prompt, then collect
    // output lines until the line holding the command (Input) that produced them.
    let mut i = lines.len();
    while i > 0 {
        let l = &lines[i - 1];
        let only_prompt = !has(l, SemanticType::Output) || line_text(l, |t| t == SemanticType::Output).is_empty();
        if only_prompt {
            i -= 1;
        } else {
            break;
        }
    }
    let mut out: Vec<String> = vec![];
    let mut command = None;
    while i > 0 {
        let l = &lines[i - 1];
        if has(l, SemanticType::Input) || has(l, SemanticType::Prompt) {
            let cmd = line_text(l, |t| t == SemanticType::Input);
            command = (!cmd.trim().is_empty()).then(|| cmd.trim().to_string());
            break;
        }
        out.push(line_text(l, |t| t == SemanticType::Output));
        i -= 1;
    }
    out.reverse();
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let truncated = out.len() > MAX_OUTPUT_LINES;
    if truncated {
        out.drain(..out.len() - MAX_OUTPUT_LINES);
    }
    Captured {
        command,
        output: out.join("\n"),
        exact: true,
        truncated,
    }
}

impl TermApp {
    /// Agents in the active workspace other than `except`.
    fn workspace_agents(&self, except: PaneId) -> Vec<PaneId> {
        let ws = Mux::get().active_workspace();
        crate::agents::collect(None)
            .into_iter()
            .filter(|a| a.workspace == ws && a.pane_id != except)
            .map(|a| a.pane_id)
            .collect()
    }

    /// From the active pane to an agent: the only one in the workspace, or
    /// pick one in the palette.
    pub(crate) fn send_output_to_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(src) = self.active_pane() else {
            return;
        };
        let targets = self.workspace_agents(src.pane_id());
        match targets.as_slice() {
            [] => self.flash("No other agent in this workspace"),
            [one] => self.send_output(src.pane_id(), *one, window, cx),
            _ => {
                self.open_palette(Scope::Agents, cx);
                if let Some(crate::app::Overlay::Palette(p)) = self.overlay.as_mut() {
                    p.send_from = Some(src.pane_id());
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn send_output(
        &mut self,
        from: PaneId,
        to: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mux = Mux::get();
        let (Some(src), Some(dst)) = (mux.get_pane(from), mux.get_pane(to)) else {
            return;
        };
        let cap = capture(&src);
        if cap.output.trim().is_empty() {
            self.flash("Nothing to send");
            cx.notify();
            return;
        }
        let pane_ref = crate::session::server_pane(from)
            .map(|p| format!("pane {p}"))
            .unwrap_or_else(|| "a pane".into());
        let cwd = src
            .get_current_working_dir(CachePolicy::AllowStale)
            .map(|u| u.path().replace(&*config::HOME_DIR.to_string_lossy(), "~"))
            .unwrap_or_default();
        let what = match (&cap.command, cap.exact) {
            (Some(cmd), _) => format!("Output of `{cmd}`"),
            (None, true) => "Output of the last command".to_string(),
            (None, false) => "Current screen".to_string(),
        };
        let note = if cap.truncated { " (last 400 lines)" } else { "" };
        let msg = format!(
            "{what} in {pane_ref} ({cwd}){note}:\n```\n{}\n```\n",
            cap.output
        );
        dst.send_paste(&msg).ok();
        self.focus_pane(to, window, cx);
    }
}
