//! Keyboard: gpui keystrokes -> WezTerm key codes, and WezTerm's
//! leader / key-table resolution against the user's config.

use config::keyassignment::{
    ClipboardCopyDestination, ClipboardPasteSource, KeyAssignment, KeyTableEntry, KeyTables,
    SpawnTabDomain,
};
use crate::commands::AppCommand;
use config::ConfigHandle;
use std::time::{Duration, Instant};
use termwiz::input::KeyCode as TermKey;
use wezterm_input_types::{KeyCode as WKey, Modifiers as Mods};

/// A keystroke, translated two ways: how WezTerm keybindings see it, and how
/// it gets encoded to the pty.
pub struct KeyInput {
    pub key: WKey,
    pub mods: Mods,
    pub term_key: Option<TermKey>,
    pub term_mods: Mods,
}

pub fn translate(ks: &gpui::Keystroke) -> Option<KeyInput> {
    let m = &ks.modifiers;
    let mut mods = Mods::NONE;
    if m.control {
        mods |= Mods::CTRL;
    }
    if m.alt {
        mods |= Mods::ALT;
    }
    if m.shift {
        mods |= Mods::SHIFT;
    }
    if m.platform {
        mods |= Mods::SUPER;
    }

    let (key, term_key) = match ks.key.as_str() {
        "enter" => (WKey::Char('\r'), TermKey::Enter),
        "escape" => (WKey::Char('\u{1b}'), TermKey::Escape),
        "backspace" => (WKey::Char('\u{8}'), TermKey::Backspace),
        "tab" => (WKey::Char('\t'), TermKey::Tab),
        "delete" => (WKey::Char('\u{7f}'), TermKey::Delete),
        "space" => (WKey::Char(' '), TermKey::Char(' ')),
        "up" => (WKey::UpArrow, TermKey::UpArrow),
        "down" => (WKey::DownArrow, TermKey::DownArrow),
        "left" => (WKey::LeftArrow, TermKey::LeftArrow),
        "right" => (WKey::RightArrow, TermKey::RightArrow),
        "home" => (WKey::Home, TermKey::Home),
        "end" => (WKey::End, TermKey::End),
        "pageup" => (WKey::PageUp, TermKey::PageUp),
        "pagedown" => (WKey::PageDown, TermKey::PageDown),
        "insert" => (WKey::Insert, TermKey::Insert),
        k if k.len() > 1 && k.starts_with('f') && k[1..].parse::<u8>().is_ok() => {
            let n = k[1..].parse::<u8>().unwrap();
            (WKey::Function(n), TermKey::Function(n))
        }
        k => {
            let mut chars = k.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            // Plain typing (no ctrl/alt/cmd): send what the layout produced,
            // e.g. shift+s -> 'S', shift+4 -> '$'.
            let typed = if !(m.control || m.alt || m.platform) {
                ks.key_char
                    .as_deref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(c)
            } else {
                c
            };
            (WKey::Char(c), TermKey::Char(typed))
        }
    };

    Some(KeyInput {
        key,
        mods,
        term_key: Some(term_key),
        term_mods: mods,
    })
}

/// Convert a `SendKey` from the config into something the pty encoder understands.
pub fn wkey_to_term(key: &WKey) -> Option<TermKey> {
    Some(match key {
        WKey::Char('\r') => TermKey::Enter,
        WKey::Char('\u{1b}') => TermKey::Escape,
        WKey::Char('\u{8}') => TermKey::Backspace,
        WKey::Char('\t') => TermKey::Tab,
        WKey::Char('\u{7f}') => TermKey::Delete,
        WKey::Char(c) => TermKey::Char(*c),
        WKey::UpArrow => TermKey::UpArrow,
        WKey::DownArrow => TermKey::DownArrow,
        WKey::LeftArrow => TermKey::LeftArrow,
        WKey::RightArrow => TermKey::RightArrow,
        WKey::Home => TermKey::Home,
        WKey::End => TermKey::End,
        WKey::PageUp => TermKey::PageUp,
        WKey::PageDown => TermKey::PageDown,
        WKey::Insert => TermKey::Insert,
        WKey::Function(n) => TermKey::Function(*n),
        _ => return None,
    })
}

struct StackEntry {
    name: String,
    one_shot: bool,
    until_unknown: bool,
    prevent_fallback: bool,
    timeout: Option<Duration>,
    expires: Option<Instant>,
}

pub enum Lookup {
    Action(KeyAssignment),
    /// The leader key was pressed; wait for the next key.
    LeaderArmed,
    /// Swallow the key (unbound key after leader, or prevent_fallback table).
    Swallow,
    /// Leader + 1–9 with nothing bound to it: jump to that agent (0-based,
    /// sidebar order).
    Agent(usize),
    /// Not bound: send it to the pane.
    Unbound,
}

pub struct KeyState {
    tables: KeyTables,
    leader: Option<(WKey, Mods, Duration)>,
    leader_until: Option<Instant>,
    stack: Vec<StackEntry>,
}

impl KeyState {
    pub fn new(config: &ConfigHandle) -> Self {
        let mut tables = config.key_bindings();
        if !config.disable_default_key_bindings {
            for ((key, mods), action) in default_bindings() {
                let k = key.normalize_shift(mods);
                tables
                    .default
                    .entry(k)
                    .or_insert(KeyTableEntry { action });
            }
        }
        let leader = config.leader.as_ref().map(|l| {
            let (k, m) = l
                .key
                .key
                .resolve(config.key_map_preference)
                .normalize_shift(l.key.mods);
            (k, m, Duration::from_millis(l.timeout_milliseconds))
        });
        Self {
            tables,
            leader,
            leader_until: None,
            stack: vec![],
        }
    }

    /// Human-readable shortcut bound to `action` in the default table, e.g. "⌘K" or "⌃A V".
    pub fn shortcut_for(&self, action: &KeyAssignment) -> Option<String> {
        let mut found: Vec<String> = self
            .tables
            .default
            .iter()
            .filter(|(_, e)| &e.action == action)
            .map(|((k, m), _)| {
                if m.contains(Mods::LEADER) {
                    let leader = self
                        .leader
                        .as_ref()
                        .map(|(lk, lm, _)| format_combo(lk, *lm))
                        .unwrap_or_else(|| "Leader".into());
                    format!("{leader} {}", format_combo(k, *m - Mods::LEADER))
                } else {
                    format_combo(k, *m)
                }
            })
            .collect();
        // prefer the shortest spelling
        found.sort_by_key(|s| (s.chars().count(), s.clone()));
        found.into_iter().next()
    }

    /// What a plain (non-leader, no key table) press is bound to, without
    /// touching leader/table state.
    pub fn peek(&self, key: &WKey, mods: Mods) -> Option<KeyAssignment> {
        candidates(key, mods)
            .iter()
            .find_map(|c| self.tables.default.get(c).map(|e| e.action.clone()))
    }

    pub fn leader_active(&self) -> bool {
        self.leader_until.is_some_and(|t| t > Instant::now())
    }

    pub fn active_table(&self) -> Option<&str> {
        self.stack.last().map(|e| e.name.as_str())
    }

    pub fn push_table(
        &mut self,
        name: String,
        timeout_milliseconds: Option<u64>,
        replace_current: bool,
        one_shot: bool,
        until_unknown: bool,
        prevent_fallback: bool,
    ) {
        if replace_current {
            self.stack.pop();
        }
        let timeout = timeout_milliseconds.map(Duration::from_millis);
        self.stack.push(StackEntry {
            name,
            one_shot,
            until_unknown,
            prevent_fallback,
            timeout,
            expires: timeout.map(|t| Instant::now() + t),
        });
    }

    pub fn pop_table(&mut self) {
        self.stack.pop();
    }

    pub fn clear_tables(&mut self) {
        self.stack.clear();
    }

    pub fn lookup(&mut self, key: &WKey, mods: Mods) -> Lookup {
        let now = Instant::now();
        self.stack.retain(|e| e.expires.is_none_or(|t| t > now));

        let leader_was_active = self.leader_active();
        self.leader_until = None;

        if !leader_was_active {
            if let Some((lk, lm, timeout)) = &self.leader {
                if key.normalize_shift(mods) == (lk.clone(), *lm) {
                    self.leader_until = Some(now + *timeout);
                    return Lookup::LeaderArmed;
                }
            }
        }
        let mods = if leader_was_active {
            mods | Mods::LEADER
        } else {
            mods
        };
        let candidates = candidates(key, mods);

        let mut idx = self.stack.len();
        while idx > 0 {
            idx -= 1;
            let found = self.tables.by_name.get(&self.stack[idx].name).and_then(|table| {
                candidates
                    .iter()
                    .find_map(|c| table.get(c).map(|e| e.action.clone()))
            });
            let entry = &mut self.stack[idx];
            if let Some(action) = found {
                if entry.one_shot {
                    self.stack.truncate(idx);
                } else if let Some(t) = entry.timeout {
                    entry.expires = Some(now + t);
                }
                return Lookup::Action(action);
            }
            if entry.one_shot || entry.until_unknown {
                self.stack.truncate(idx);
                continue;
            }
            if entry.prevent_fallback {
                return Lookup::Swallow;
            }
        }

        if let Some(action) = candidates
            .iter()
            .find_map(|c| self.tables.default.get(c).map(|e| e.action.clone()))
        {
            return Lookup::Action(action);
        }
        if leader_was_active {
            match key {
                WKey::Char(c @ '1'..='9') if mods == Mods::LEADER => {
                    Lookup::Agent(*c as usize - '1' as usize)
                }
                _ => Lookup::Swallow,
            }
        } else {
            Lookup::Unbound
        }
    }
}

fn candidates(key: &WKey, mods: Mods) -> Vec<(WKey, Mods)> {
    let mut v = vec![key.normalize_shift(mods)];
    // Shifted punctuation: bindings like {key="$", mods="LEADER"} don't mention SHIFT.
    if mods.contains(Mods::SHIFT) {
        if let WKey::Char(c) = key {
            if !c.is_ascii_alphabetic() {
                v.push((key.clone(), mods - Mods::SHIFT));
            }
        }
    }
    v
}

/// The subset of WezTerm's built-in macOS defaults that this shell implements.
/// Anything in the user's config takes precedence.
fn default_bindings() -> Vec<((WKey, Mods), KeyAssignment)> {
    use config::keyassignment::PaneDirection;
    use KeyAssignment::*;
    let s = Mods::SUPER;
    let mut v = vec![
        (
            (WKey::Char('c'), s),
            CopyTo(ClipboardCopyDestination::Clipboard),
        ),
        (
            (WKey::Char('v'), s),
            PasteFrom(ClipboardPasteSource::Clipboard),
        ),
        (
            (WKey::Char('t'), s),
            SpawnTab(SpawnTabDomain::CurrentPaneDomain),
        ),
        ((WKey::Char('{'), s), ActivateTabRelative(-1)),
        ((WKey::Char('}'), s), ActivateTabRelative(1)),
        ((WKey::Char('='), s), IncreaseFontSize),
        ((WKey::Char('-'), s), DecreaseFontSize),
        ((WKey::Char('0'), s), ResetFontSize),
        ((WKey::Char('r'), s | Mods::SHIFT), ReloadConfiguration),
        ((WKey::Char('m'), s), Hide),
        ((WKey::Char('q'), s), QuitApplication),
        ((WKey::Char(' '), Mods::CTRL | Mods::SHIFT), QuickSelect),
        // panes
        ((WKey::Char('w'), s), CloseCurrentPane { confirm: true }),
        ((WKey::Char('\r'), s), TogglePaneZoomState),
        ((WKey::LeftArrow, s | Mods::ALT), ActivatePaneDirection(PaneDirection::Left)),
        ((WKey::RightArrow, s | Mods::ALT), ActivatePaneDirection(PaneDirection::Right)),
        ((WKey::UpArrow, s | Mods::ALT), ActivatePaneDirection(PaneDirection::Up)),
        ((WKey::DownArrow, s | Mods::ALT), ActivatePaneDirection(PaneDirection::Down)),
        // app-level (see commands.rs); bind these in Lua to move them
        ((WKey::Char('b'), s), AppCommand::ToggleSidebar.action()),
        ((WKey::Char('k'), s), AppCommand::CommandPalette.action()),
        ((WKey::Char('P'), s), AppCommand::CommandPalette.action()),
        ((WKey::Char('n'), s), AppCommand::NewAgent.action()),
        ((WKey::Char('N'), s), AppCommand::NewAgentTab.action()),
        ((WKey::Char('d'), s), AppCommand::ToggleDiffPanel.action()),
        ((WKey::Char('r'), s), AppCommand::RunRecipe.action()),
        ((WKey::Char('D'), s), AppCommand::ToggleDiffPanel.action()),
        ((WKey::Char('e'), s), AppCommand::OpenEditor.action()),
        ((WKey::Char(','), s), AppCommand::OpenSettings.action()),
        ((WKey::Char('\r'), s | Mods::SHIFT), AppCommand::SendOutputToAgent.action()),
    ];
    for i in 1..=9u8 {
        let c = (b'0' + i) as char;
        v.push(((WKey::Char(c), s), ActivateTab(i as isize - 1)));
    }
    v
}

fn format_combo(key: &WKey, mods: Mods) -> String {
    let mut out = String::new();
    for (m, sym) in [
        (Mods::CTRL, "⌃"),
        (Mods::ALT, "⌥"),
        (Mods::SHIFT, "⇧"),
        (Mods::SUPER, "⌘"),
    ] {
        if mods.contains(m) {
            out.push_str(sym);
        }
    }
    let k = match key {
        WKey::Char('\r') => "↵".to_string(),
        WKey::Char('\t') => "⇥".to_string(),
        WKey::Char('\u{1b}') => "Esc".to_string(),
        WKey::Char('\u{8}') => "⌫".to_string(),
        WKey::Char(' ') => "Space".to_string(),
        WKey::Char(c) => c.to_uppercase().to_string(),
        WKey::UpArrow => "↑".into(),
        WKey::DownArrow => "↓".into(),
        WKey::LeftArrow => "←".into(),
        WKey::RightArrow => "→".into(),
        WKey::Function(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    out.push_str(&k);
    out
}
