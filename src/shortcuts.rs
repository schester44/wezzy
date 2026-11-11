//! The global shortcut sheet (Leader ?), built from the live key table so it
//! shows the user's own bindings, not just ours.

use crate::commands::AppCommand;
use crate::keys::KeyState;
use config::keyassignment::{KeyAssignment, LauncherFlags, PaneDirection, PaneSelectMode};

pub struct Row {
    /// Key caps shown side by side: alternatives, or a shared modifier followed
    /// by the keys it applies to.
    pub keys: Vec<String>,
    pub label: String,
}

pub struct Group {
    pub title: String,
    pub rows: Vec<Row>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Section {
    App,
    Panes,
    Terminal,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::App => "Wezzy",
            Section::Panes => "Tabs & panes",
            Section::Terminal => "Terminal",
        }
    }
}

/// Columns of groups: the leader's bindings first, then everything else by section.
pub fn sheet(keys: &KeyState) -> Vec<Vec<Group>> {
    let mut leader: Vec<(String, String)> = vec![];
    let mut other: Vec<(Section, String, String)> = vec![];
    for (is_leader, key, action) in keys.bindings() {
        let Some((section, label)) = describe(action) else { continue };
        if is_leader {
            leader.push((key, label));
        } else {
            other.push((section, label, key));
        }
    }

    let mut columns = vec![];
    if let Some(lk) = keys.leader_label() {
        leader.sort_by(|a, b| sort_key(&a.0).cmp(&sort_key(&b.0)).then(a.1.cmp(&b.1)));
        let rows = merge(leader.into_iter().map(|(k, l)| (l, k)).collect(), false);
        columns.push(vec![Group {
            title: format!("Leader  {lk}"),
            rows,
        }]);
    }

    // ⌥⌘1–9 jumps to an agent unless the user bound those keys
    if !other.iter().any(|(_, _, k)| k.starts_with("⌥⌘") && k.chars().last().is_some_and(|c| c.is_ascii_digit())) {
        other.push((Section::App, "Go to agent".into(), "⌥⌘1–9".into()));
    }
    other.sort();
    let mut groups: Vec<Group> = vec![];
    for section in [Section::App, Section::Panes, Section::Terminal] {
        let entries: Vec<(String, String)> = other
            .iter()
            .filter(|(s, ..)| *s == section)
            .map(|(_, l, k)| (l.clone(), k.clone()))
            .collect();
        if !entries.is_empty() {
            groups.push(Group {
                title: section.title().into(),
                rows: merge(entries, true),
            });
        }
    }
    // the first section gets its own column; the rest fill whichever is shorter
    let mut rest = vec![vec![], vec![]];
    for (i, g) in groups.into_iter().enumerate() {
        let len = |c: &Vec<Group>| c.iter().map(|g| g.rows.len() + 2).sum::<usize>();
        let col = if i == 0 || len(&rest[0]) <= len(&rest[1]) { 0 } else { 1 };
        rest[col].push(g);
    }
    columns.extend(rest.into_iter().filter(|c| !c.is_empty()));
    columns
}

/// Lowercase before uppercase, letters before punctuation: the order you'd scan for.
fn sort_key(k: &str) -> (bool, String, bool) {
    let shifted = k.starts_with('⇧');
    let base = k.trim_start_matches('⇧').to_lowercase();
    let punct = !base.chars().next().is_some_and(|c| c.is_alphanumeric());
    (punct, base, shifted)
}

/// One row per label, its keys collapsed: ⌘1 … ⌘9 becomes ⌘1–9, and three or
/// more keys sharing modifiers show the modifiers once (⌥⌘ ← → ↑ ↓).
/// `entries` is (label, key), sorted when `sorted_labels`.
fn merge(entries: Vec<(String, String)>, sorted_labels: bool) -> Vec<Row> {
    let mut rows: Vec<(String, Vec<String>)> = vec![];
    for (label, key) in entries {
        match rows.iter_mut().find(|(l, _)| *l == label) {
            Some((_, keys)) => keys.push(key),
            None => rows.push((label, vec![key])),
        }
    }
    if sorted_labels {
        rows.sort_by(|a, b| a.0.cmp(&b.0));
    }
    rows.into_iter()
        .map(|(label, mut keys)| {
            keys.sort();
            keys.dedup();
            Row { keys: collapse(keys), label }
        })
        .collect()
}

fn collapse(keys: Vec<String>) -> Vec<String> {
    if keys.len() < 3 {
        return keys;
    }
    let split = |k: &str| {
        let i = k.char_indices().last().map_or(0, |(i, _)| i);
        (k[..i].to_string(), k[i..].to_string())
    };
    let parts: Vec<(String, String)> = keys.iter().map(|k| split(k)).collect();
    let prefix = &parts[0].0;
    if !parts.iter().all(|(p, _)| p == prefix) {
        return keys;
    }
    let digits: Option<Vec<u32>> = parts.iter().map(|(_, s)| s.parse().ok()).collect();
    if let Some(mut d) = digits {
        d.sort();
        if d.windows(2).all(|w| w[1] == w[0] + 1) {
            return vec![format!("{prefix}{}–{}", d[0], d[d.len() - 1])];
        }
    }
    if prefix.is_empty() {
        return keys;
    }
    std::iter::once(prefix.clone())
        .chain(parts.into_iter().map(|(_, s)| s))
        .collect()
}

fn dir(d: &PaneDirection) -> &'static str {
    match d {
        PaneDirection::Up => "up",
        PaneDirection::Down => "down",
        PaneDirection::Left => "left",
        PaneDirection::Right => "right",
        PaneDirection::Next => "next",
        PaneDirection::Prev => "previous",
    }
}

/// What a binding does, in the sheet's words. Only actions `perform()` handles;
/// Lua callbacks and no-ops are opaque, so they're left out.
fn describe(action: &KeyAssignment) -> Option<(Section, String)> {
    use KeyAssignment::*;
    use Section::*;
    let s = |sec: Section, l: &str| Some((sec, l.to_string()));
    match action {
        EmitEvent(name) => AppCommand::from_event(name).and_then(|c| s(App, c.label())),
        ActivateCommandPalette | ShowLauncher => s(App, "Command palette"),
        ShowLauncherArgs(a) if a.flags.contains(LauncherFlags::WORKSPACES) => {
            s(App, "Switch workspace")
        }
        ShowLauncherArgs(_) => s(App, "Command palette"),
        SwitchToWorkspace { name: Some(n), .. } => Some((App, format!("Workspace {n}"))),
        SwitchToWorkspace { .. } => s(App, "New workspace"),
        SwitchWorkspaceRelative(n) if *n < 0 => s(App, "Previous workspace"),
        SwitchWorkspaceRelative(_) => s(App, "Next workspace"),
        ReloadConfiguration => s(App, "Reload config"),
        ToggleFullScreen => s(App, "Full screen"),
        Hide => s(App, "Hide"),
        QuitApplication => s(App, "Quit"),
        PromptInputLine(p) => {
            let d = crate::app::strip_escapes(&p.description);
            let d = d.trim().trim_end_matches(':').trim();
            (!d.is_empty()).then(|| (App, d.to_string()))
        }
        ActivateKeyTable { name, .. } => {
            let mut n = name.replace(['_', '-'], " ");
            if let Some(c) = n.get(..1) {
                n.replace_range(..1, &c.to_uppercase());
            }
            Some((Panes, format!("{n} mode")))
        }

        SpawnTab(_) | SpawnCommandInNewTab(_) => s(Panes, "New tab"),
        ActivateTab(_) => s(Panes, "Go to tab"),
        ActivateTabRelative(n) if *n < 0 => s(Panes, "Previous tab"),
        ActivateTabRelative(_) => s(Panes, "Next tab"),
        ActivateLastTab => s(Panes, "Last tab"),
        ShowTabNavigator => s(Panes, "Find tab"),
        SpawnWindow => s(Panes, "New window"),
        ActivateWindow(_) => s(Panes, "Go to window"),
        ActivateWindowRelative(n) | ActivateWindowRelativeNoWrap(n) if *n < 0 => {
            s(Panes, "Previous window")
        }
        ActivateWindowRelative(_) | ActivateWindowRelativeNoWrap(_) => s(Panes, "Next window"),
        MoveTab(_) => s(Panes, "Move tab to"),
        MoveTabRelative(n) if *n < 0 => s(Panes, "Move tab left"),
        MoveTabRelative(_) => s(Panes, "Move tab right"),
        CloseCurrentTab { .. } => s(Panes, "Close tab"),
        CloseCurrentPane { .. } => s(Panes, "Close pane"),
        SplitHorizontal(_) => s(Panes, "Split right"),
        SplitVertical(_) => s(Panes, "Split down"),
        SplitPane(sp) => Some((Panes, format!("Split {}", dir(&sp.direction)))),
        ActivatePaneDirection(_) | ActivatePaneByIndex(_) => s(Panes, "Focus pane"),
        AdjustPaneSize(..) => s(Panes, "Resize pane"),
        TogglePaneZoomState | SetPaneZoomState(_) => s(Panes, "Zoom pane"),
        RotatePanes(_) => s(Panes, "Rotate panes"),
        PaneSelect(a) => s(
            Panes,
            match a.mode {
                PaneSelectMode::Activate => "Pick a pane",
                PaneSelectMode::SwapWithActive | PaneSelectMode::SwapWithActiveKeepFocus => {
                    "Swap panes"
                }
                PaneSelectMode::MoveToNewTab => "Move pane to a tab",
                PaneSelectMode::MoveToNewWindow => "Move pane to a window",
            },
        ),

        CopyTo(_) | CopyTextTo { .. } => s(Terminal, "Copy"),
        PasteFrom(_) => s(Terminal, "Paste"),
        QuickSelect | QuickSelectArgs(_) => s(Terminal, "Quick select"),
        ActivateCopyMode => s(Terminal, "Copy mode"),
        SendKey(_) => s(Terminal, "Send key"),
        ScrollByPage(n) if n.into_inner() < 0. => s(Terminal, "Scroll up"),
        ScrollByPage(_) => s(Terminal, "Scroll down"),
        ScrollByLine(n) if *n < 0 => s(Terminal, "Scroll up a line"),
        ScrollByLine(_) => s(Terminal, "Scroll down a line"),
        ScrollToTop => s(Terminal, "Scroll to top"),
        ScrollToBottom => s(Terminal, "Scroll to bottom"),
        ClearScrollback(_) => s(Terminal, "Clear scrollback"),
        ResetTerminal => s(Terminal, "Reset terminal"),
        IncreaseFontSize => s(Terminal, "Bigger text"),
        DecreaseFontSize => s(Terminal, "Smaller text"),
        ResetFontSize => s(Terminal, "Reset text size"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::collapse;

    fn v(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn collapses_digit_runs() {
        let keys: Vec<String> = (1..=9).map(|i| format!("⌘{i}")).collect();
        assert_eq!(collapse(keys), v(&["⌘1–9"]));
    }

    #[test]
    fn shares_modifiers() {
        assert_eq!(collapse(v(&["⌥⌘←", "⌥⌘↑", "⌥⌘→"])), v(&["⌥⌘", "←", "↑", "→"]));
    }

    #[test]
    fn leaves_pairs_alone() {
        assert_eq!(collapse(v(&["⌘K", "⌘P"])), v(&["⌘K", "⌘P"]));
    }
}
