//! Settings panel (⌘, or ⌘K → Settings). Vim keys: j/k move, h/l change,
//! ↵ toggle / edit / run, esc close.

use crate::app::{Overlay, TermApp};
use crate::settings::{self, AgentLayout, CloseConfirmation};
use gpui::{Context, KeyDownEvent, Window};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    DefaultAgent,
    AgentLayout,
    CloseConfirmation,
    Editor,
    Sidebar,
    Sound,
    AgentHooks,
    ConfigFile,
}

pub const ROWS: &[Row] = &[
    Row::DefaultAgent,
    Row::AgentLayout,
    Row::CloseConfirmation,
    Row::Editor,
    Row::Sidebar,
    Row::Sound,
    Row::AgentHooks,
    Row::ConfigFile,
];

pub struct SettingsState {
    pub cursor: usize,
    /// Inline text edit in progress (Editor row).
    pub editing: Option<String>,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::DefaultAgent => "Default agent",
            Row::AgentLayout => "Agent layout",
            Row::CloseConfirmation => "Confirm close",
            Row::Editor => "Editor",
            Row::Sidebar => "Sidebar",
            Row::Sound => "Sound",
            Row::AgentHooks => "Agent hooks",
            Row::ConfigFile => "Config file",
        }
    }

    /// Current value as shown.
    pub fn value(self, sidebar_open: bool) -> String {
        let s = settings::get();
        match self {
            Row::DefaultAgent => s
                .harness(&s.default_harness_name())
                .map(|h| h.label())
                .unwrap_or_else(|| s.default_harness_name()),
            Row::AgentLayout => match s.agent_layout {
                AgentLayout::Bsp => "BSP".into(),
                AgentLayout::MasterStack => "Master stack".into(),
            },
            Row::CloseConfirmation => match s.close_confirmation {
                CloseConfirmation::Smart => "When busy".into(),
                CloseConfirmation::Never => "Never".into(),
                CloseConfirmation::Always => "Always".into(),
            },
            Row::Editor => {
                let argv: Vec<String> = s
                    .editor()
                    .into_iter()
                    .filter(|a| !a.contains('{'))
                    .collect();
                shell_words::join(argv)
            }
            Row::Sidebar => if sidebar_open { "Shown" } else { "Hidden" }.into(),
            Row::Sound => s.sound().unwrap_or_else(|| "Off".into()),
            Row::AgentHooks => {
                if crate::hooks::claude_hooks_installed() {
                    "Installed".into()
                } else {
                    "Install".into()
                }
            }
            Row::ConfigFile => "Open".into(),
        }
    }

    /// Choice rows cycle; the rest act on ↵.
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Row::DefaultAgent
                | Row::AgentLayout
                | Row::CloseConfirmation
                | Row::Sidebar
                | Row::Sound
        )
    }
}

fn cycle<T: Copy + PartialEq>(items: &[T], cur: T, step: isize) -> T {
    let i = items.iter().position(|x| *x == cur).unwrap_or(0) as isize;
    let n = items.len() as isize;
    items[(i + step).rem_euclid(n) as usize]
}

impl TermApp {
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Settings(SettingsState {
            cursor: 0,
            editing: None,
        }));
        cx.notify();
    }

    fn change_setting(&mut self, row: Row, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        match row {
            Row::DefaultAgent => {
                let names: Vec<String> = settings::get().harnesses().into_iter().map(|h| h.name).collect();
                if names.is_empty() {
                    return;
                }
                let cur = settings::get().default_harness_name();
                let i = names.iter().position(|n| *n == cur).unwrap_or(0) as isize;
                let next = names[(i + step).rem_euclid(names.len() as isize) as usize].clone();
                settings::update(|s| s.default_harness = next);
            }
            Row::AgentLayout => settings::update(|s| {
                s.agent_layout = cycle(&[AgentLayout::Bsp, AgentLayout::MasterStack], s.agent_layout, step)
            }),
            Row::CloseConfirmation => settings::update(|s| {
                s.close_confirmation = cycle(
                    &[CloseConfirmation::Smart, CloseConfirmation::Never, CloseConfirmation::Always],
                    s.close_confirmation,
                    step,
                )
            }),
            Row::Sidebar => {
                self.sidebar_open = !self.sidebar_open;
                let open = self.sidebar_open;
                settings::update(|s| s.show_sidebar = open);
            }
            Row::Sound => {
                let cur = settings::get().sound().unwrap_or_else(|| "off".into());
                let next = cycle(crate::sound::CHOICES, cur.as_str(), step).to_string();
                settings::update(|s| s.sound = next.clone());
                crate::sound::play(&next);
            }
            Row::Editor => {
                if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
                    st.editing = Some(Row::Editor.value(true));
                }
            }
            Row::AgentHooks => {
                self.run_app_command(crate::commands::AppCommand::InstallAgentHooks, window, cx)
            }
            Row::ConfigFile => {
                self.overlay = None;
                let path = crate::paths::config_dir().join("dekita.lua");
                self.open_in_editor(&path, None, None, window, cx);
            }
        }
        cx.notify();
    }

    /// Row clicked with the mouse.
    pub(crate) fn settings_click(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Settings(st)) = self.overlay.as_mut() {
            st.cursor = idx;
        }
        self.change_setting(ROWS[idx], 1, window, cx);
    }

    pub(crate) fn settings_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(Overlay::Settings(st)) = self.overlay.as_mut() else {
            return;
        };

        // inline text editing
        if let Some(text) = st.editing.as_mut() {
            match ks.key.as_str() {
                "escape" => st.editing = None,
                "enter" => {
                    let value = std::mem::take(text);
                    st.editing = None;
                    let argv = shell_words::split(value.trim()).unwrap_or_default();
                    settings::update(|s| s.editor = argv);
                }
                "backspace" if ks.modifiers.alt || ks.modifiers.platform => text.clear(),
                "backspace" => {
                    text.pop();
                }
                "v" if ks.modifiers.platform => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        text.push_str(t.lines().next().unwrap_or(""));
                    }
                }
                _ if !ks.modifiers.control && !ks.modifiers.platform => {
                    if let Some(c) = &ks.key_char {
                        text.push_str(c);
                    }
                }
                _ => {}
            }
            return;
        }

        let n = ROWS.len();
        let row = ROWS[st.cursor.min(n - 1)];
        match ks.key.as_str() {
            "escape" | "q" => self.overlay = None,
            "[" if ks.modifiers.control => self.overlay = None,
            "j" | "down" => st.cursor = (st.cursor + 1).min(n - 1),
            "k" | "up" => st.cursor = st.cursor.saturating_sub(1),
            "g" if !ks.modifiers.shift => st.cursor = 0,
            "g" | "G" => st.cursor = n - 1,
            "l" | "right" if row.is_choice() => self.change_setting(row, 1, window, cx),
            "h" | "left" if row.is_choice() => self.change_setting(row, -1, window, cx),
            "enter" | "space" => self.change_setting(row, 1, window, cx),
            _ => {}
        }
    }
}
