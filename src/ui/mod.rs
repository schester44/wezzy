//! Visual layer: theme tokens derived from the WezTerm palette, icons, and the view.

mod git;
mod help;
mod modal;
mod palette;
mod panel;
mod queue;
mod settings;
mod sidebar;
mod terminal;
mod topbar;
mod view;
mod widgets;
pub use panel::SidePanel;
pub(crate) use help::group as shortcut_group;
pub(crate) use widgets::{term_family, with_alpha};

use gpui::{px, svg, AssetSource, Hsla, IntoElement, Rgba, SharedString, Styled};
use std::borrow::Cow;
use wezterm_term::color::{ColorPalette, SrgbaTuple};

// ---------------------------------------------------------------------------
// Icons: tiny stroke SVGs compiled into the binary.

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        macro_rules! icons {
            ($($name:literal),*) => {
                match path {
                    $(concat!("icons/", $name, ".svg") => Some(Cow::Borrowed(
                        include_bytes!(concat!("../../assets/icons/", $name, ".svg")).as_slice(),
                    )),)*
                    _ => None,
                }
            };
        }
        Ok(icons!(
            "plus",
            "x",
            "chevron-down",
            "check",
            "sidebar",
            "terminal",
            "bot",
            "layers",
            "spinner",
            "search",
            "bolt",
            "window",
            "alert",
            "home",
            "external",
            "chevron-right",
            "chevron-left",
            "git-branch",
            "git-commit",
            "sparkles",
            "message",
            "server",
            "database",
            "git-pull-request",
            "git-pull-request-draft",
            "git-pull-request-closed",
            "git-merge",
            "cloud-off",
            "code",
            "shield"
        ))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

#[derive(Clone, Copy)]
pub enum Icon {
    Plus,
    X,
    ChevronDown,
    Check,
    Bot,
    Terminal,
    Layers,
    Search,
    Bolt,
    Window,
    Alert,
    Home,
    External,
    ChevronRight,
    ChevronLeft,
    GitBranch,
    GitCommit,
    Sparkles,
    Message,
    Server,
    Database,
    PullRequest,
    PullRequestDraft,
    PullRequestClosed,
    GitMerge,
    CloudOff,
    Code,
    Shield,
}

/// A spinning loader, for "working" states.
pub fn spinner(size: f32, color: Hsla) -> impl IntoElement {
    use gpui::{percentage, Animation, AnimationExt, Transformation};
    svg()
        .path("icons/spinner.svg")
        .size(px(size))
        .flex_none()
        .text_color(color)
        .with_animation(
            "spinner",
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
        )
}

pub fn icon(icon: Icon, size: f32, color: Hsla) -> impl IntoElement {
    let path = match icon {
        Icon::Plus => "icons/plus.svg",
        Icon::X => "icons/x.svg",
        Icon::ChevronDown => "icons/chevron-down.svg",
        Icon::Check => "icons/check.svg",
        Icon::Bot => "icons/bot.svg",
        Icon::Terminal => "icons/terminal.svg",
        Icon::Layers => "icons/layers.svg",
        Icon::Search => "icons/search.svg",
        Icon::Bolt => "icons/bolt.svg",
        Icon::Window => "icons/window.svg",
        Icon::Alert => "icons/alert.svg",
        Icon::Home => "icons/home.svg",
        Icon::External => "icons/external.svg",
        Icon::ChevronRight => "icons/chevron-right.svg",
        Icon::ChevronLeft => "icons/chevron-left.svg",
        Icon::GitBranch => "icons/git-branch.svg",
        Icon::GitCommit => "icons/git-commit.svg",
        Icon::Sparkles => "icons/sparkles.svg",
        Icon::Message => "icons/message.svg",
        Icon::Server => "icons/server.svg",
        Icon::Database => "icons/database.svg",
        Icon::PullRequest => "icons/git-pull-request.svg",
        Icon::PullRequestDraft => "icons/git-pull-request-draft.svg",
        Icon::PullRequestClosed => "icons/git-pull-request-closed.svg",
        Icon::GitMerge => "icons/git-merge.svg",
        Icon::CloudOff => "icons/cloud-off.svg",
        Icon::Code => "icons/code.svg",
        Icon::Shield => "icons/shield.svg",
    };
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

// ---------------------------------------------------------------------------
// Theme: every chrome color is derived from the terminal palette, so the UI
// follows whatever color scheme the WezTerm config sets.

#[derive(Clone, Copy)]
pub struct Theme {
    /// Terminal surface (palette background at the configured opacity).
    pub term_bg: Hsla,
    /// Window chrome: sidebar + tab strip.
    pub chrome: Hsla,
    /// Raised surfaces: popovers, modals, toasts.
    pub elevated: Hsla,
    pub hover: Hsla,
    /// Active tab: the active pane's fill, so the tab and its pane read as one surface.
    pub tab_active: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub fg: Hsla,
    pub fg_muted: Hsla,
    pub fg_subtle: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
}

pub fn color(c: SrgbaTuple) -> Hsla {
    Rgba {
        r: c.0,
        g: c.1,
        b: c.2,
        a: c.3,
    }
    .into()
}

fn alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

fn shade(mut c: Hsla, l: f32) -> Hsla {
    c.l = (c.l * l).clamp(0.0, 1.0);
    c
}

impl Theme {
    pub fn new(p: &ColorPalette, opacity: f32) -> Self {
        let bg = color(p.background);
        let fg = color(p.foreground);
        let opacity = opacity.clamp(0.3, 1.0);
        Self {
            term_bg: alpha(bg, opacity),
            chrome: alpha(shade(bg, 0.72), (opacity + 0.08).min(1.0)),
            elevated: alpha(shade(bg, 1.25).max_l(0.14), 0.98),
            hover: alpha(fg, 0.05),
            tab_active: alpha(bg, opacity),
            selected: alpha(fg, 0.10),
            border: alpha(fg, 0.08),
            fg: alpha(fg, 0.92),
            fg_muted: alpha(fg, 0.58),
            fg_subtle: alpha(fg, 0.36),
            // ANSI blue: the focus/selection color (Vercel-style), never magenta
            accent: color(p.colors.0[4]),
            success: color(p.colors.0[2]),
            warning: color(p.colors.0[3]),
            danger: color(p.colors.0[1]),
        }
    }
}

trait MaxL {
    fn max_l(self, l: f32) -> Self;
}
impl MaxL for Hsla {
    fn max_l(mut self, l: f32) -> Self {
        self.l = self.l.max(l);
        self
    }
}
