//! Launching agent harnesses: ⌘I splits the current tab (bsp / master-stack),
//! ⌘⇧I opens a new tab; the palette lists every harness.

use crate::app::{send_event, AppEvent, TermApp};
use crate::settings::{self, AgentLayout, Harness};
use config::keyassignment::{SpawnCommand, SpawnTabDomain};
use gpui::{Context, Window};
use mux::tab::{SplitDirection, SplitRequest, SplitSize};
use mux::Mux;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    Split,
    Tab,
}

impl TermApp {
    pub(crate) fn launch_harness(
        &mut self,
        name: Option<&str>,
        placement: Placement,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let s = settings::get();
        let name = name.map(str::to_string).unwrap_or_else(|| s.default_harness_name());
        let Some(harness) = s.harness(&name) else {
            self.flash(format!("No harness named \"{name}\" (is it on PATH?)"));
            cx.notify();
            return;
        };
        self.spawn_harness(&harness, placement, _window, cx);
    }

    /// Run a harness (possibly with extra args, e.g. a recipe's prompt) in a
    /// split or a new tab.
    pub(crate) fn spawn_harness(
        &mut self,
        harness: &Harness,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match placement {
            Placement::Tab => {
                let spawn = spawn_command(harness, self.on_host());
                self.spawn_tab(SpawnTabDomain::CurrentPaneDomain, Some(spawn), None)
            }
            Placement::Split => self.run_in_split(harness, window, cx),
        }
        cx.notify();
    }

    /// Open a file (⌘-click on file:line) with the configured editor, in a split.
    pub(crate) fn open_in_editor(
        &mut self,
        path: &std::path::Path,
        line: Option<u32>,
        col: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file = path.display().to_string();
        let argv: Vec<String> = settings::get()
            .editor()
            .into_iter()
            .map(|a| {
                a.replace("{file}", &file)
                    .replace("{line}", &line.unwrap_or(1).to_string())
                    .replace("{col}", &col.unwrap_or(1).to_string())
            })
            .collect();
        let harness = Harness {
            name: "editor".into(),
            argv,
        };
        self.run_in_split(&harness, window, cx);
    }

    /// ⌘E: the editor in the active pane's directory, in a split.
    pub(crate) fn open_editor_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // the template without its file/line args
        let argv: Vec<String> = settings::get()
            .editor()
            .into_iter()
            .filter(|a| !a.contains("{file}") && !a.contains("{line}") && !a.contains("{col}"))
            .collect();
        let harness = Harness {
            name: "editor".into(),
            argv,
        };
        self.run_in_split(&harness, window, cx);
    }

    pub(crate) fn run_command_in_split(&mut self, harness: &Harness, window: &mut Window, cx: &mut Context<Self>) {
        self.run_in_split(harness, window, cx)
    }

    /// ⌘\: a shell in a split of the current tab, placed like a new agent.
    pub(crate) fn new_terminal(&mut self, cx: &mut Context<Self>) {
        self.spawn_in_new_pane(SpawnCommand::default(), cx)
    }

    /// A command in a split of the current tab, placed like a new agent; a
    /// shell when it has no args (`SpawnCommandInNewPane`). In a new tab when
    /// there's nowhere to split.
    pub(crate) fn spawn_in_new_pane(&mut self, spawn: SpawnCommand, cx: &mut Context<Self>) {
        let split = self.next_split.take();
        let Some((target, request)) = split.or_else(|| self.agent_split(settings::get().agent_layout)) else {
            self.spawn_tab(spawn.domain.clone(), Some(spawn), None);
            return;
        };
        let domain = spawn.domain.clone();
        let (command, command_dir) = crate::app::build_command(Some(&spawn));
        promise::spawn::spawn(async move {
            match Mux::get()
                .split_pane(target, request, mux::domain::SplitSource::Spawn { command, command_dir }, domain)
                .await
            {
                Ok((pane, _)) => pane.set_config(std::sync::Arc::new(config::TermConfig::new())),
                Err(e) => send_event(AppEvent::Flash(format!("split failed: {e:#}"))),
            }
        })
        .detach();
        cx.notify();
    }

    /// Whether the active pane runs on a host (where new splits and tabs go).
    fn on_host(&self) -> bool {
        self.active_pane().is_some_and(|p| crate::session::on_host(&p))
    }

    fn run_in_split(&mut self, harness: &Harness, _window: &mut Window, cx: &mut Context<Self>) {
        let spawn = spawn_command(harness, self.on_host());
        let split = self.next_split.take();
        let Some((target, request)) = split.or_else(|| self.agent_split(settings::get().agent_layout)) else {
            self.spawn_tab(SpawnTabDomain::CurrentPaneDomain, Some(spawn), None);
            return;
        };
        promise::spawn::spawn(async move {
            if let Err(e) = Mux::get()
                .split_pane(
                    target,
                    request,
                    mux::domain::SplitSource::Spawn {
                        command: Some(builder(&spawn)),
                        command_dir: None,
                    },
                    SpawnTabDomain::CurrentPaneDomain,
                )
                .await
            {
                send_event(AppEvent::Flash(format!("couldn't open: {e:#}")));
            }
        })
        .detach();
        cx.notify();
    }

    /// Which pane to split, and how, for the configured layout.
    fn agent_split(&self, layout: AgentLayout) -> Option<(mux::pane::PaneId, SplitRequest)> {
        let tab = self.active_tab()?;
        let panes = tab.iter_panes();
        let geom = self.grid.get();
        let (cw, lh) = (f32::from(geom.cell_w).max(1.), f32::from(geom.line_h).max(1.));
        let req = |direction| SplitRequest {
            direction,
            target_is_second: true,
            top_level: false,
            size: SplitSize::Percent(50),
        };
        match layout {
            AgentLayout::Bsp => {
                let p = panes.iter().find(|p| p.is_active)?;
                // alternate with the split that made this pane, so halving a
                // column stacks inside it instead of opening a column between;
                // a lone pane splits along its longer side (pixels, not cells)
                let wide = match parent_split(&tab.codec_pane_tree(), p.pane.pane_id()) {
                    Some(SplitDirection::Horizontal) => false,
                    Some(SplitDirection::Vertical) => true,
                    None => p.width as f32 * cw >= p.height as f32 * lh,
                };
                Some((
                    p.pane.pane_id(),
                    req(if wide {
                        SplitDirection::Horizontal
                    } else {
                        SplitDirection::Vertical
                    }),
                ))
            }
            AgentLayout::MasterStack => {
                if panes.len() == 1 {
                    return Some((panes[0].pane.pane_id(), req(SplitDirection::Horizontal)));
                }
                // bottom of the right-most column
                let p = panes.iter().max_by_key(|p| (p.left, p.top))?;
                Some((p.pane.pane_id(), req(SplitDirection::Vertical)))
            }
        }
    }
}

/// Direction of the split whose direct child is `pane`, if it has one.
fn parent_split(node: &mux::tab::PaneNode, pane: mux::pane::PaneId) -> Option<SplitDirection> {
    use mux::tab::PaneNode;
    let PaneNode::Split { left, right, node } = node else {
        return None;
    };
    let is_leaf = |n: &PaneNode| matches!(n, PaneNode::Leaf(e) if e.pane_id == pane);
    if is_leaf(left) || is_leaf(right) {
        return Some(node.direction);
    }
    parent_split(left, pane).or_else(|| parent_split(right, pane))
}

/// Run the harness through the login shell (so PATH/env from your dotfiles
/// apply) and drop back to a shell when it exits. On a host, that's the
/// host's login shell, which only the host knows: `$SHELL` there (the client
/// domain sends host commands without this Mac's environment).
fn spawn_command(h: &Harness, on_host: bool) -> SpawnCommand {
    let cmd = shell_words::join(&h.argv);
    let mut env = std::collections::HashMap::new();
    env.insert("WEZZY_HARNESS".to_string(), h.name.clone());
    let args = if on_host {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            r#"exec "$SHELL" -lic "$0; exec \"$SHELL\" -l""#.into(),
            cmd,
        ]
    } else {
        let shell = login_shell();
        vec![shell.clone(), "-lic".into(), format!("{cmd}; exec {} -l", shell_words::quote(&shell))]
    };
    SpawnCommand {
        label: Some(h.label()),
        args: Some(args),
        set_environment_variables: env,
        domain: SpawnTabDomain::CurrentPaneDomain,
        ..Default::default()
    }
}

fn builder(spawn: &SpawnCommand) -> portable_pty::CommandBuilder {
    let mut b = portable_pty::CommandBuilder::from_argv(
        spawn
            .args
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect(),
    );
    for (k, v) in &spawn.set_environment_variables {
        b.env(k, v);
    }
    b
}

fn login_shell() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            if let Ok(s) = std::ffi::CStr::from_ptr((*pw).pw_shell).to_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    "/bin/zsh".into()
}
