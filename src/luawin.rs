//! A stand-in for WezTerm's GUI `window` object, so `wezterm.action_callback`
//! bindings and `wezterm.on(...)` handlers from the user's config can run here.
//! Mux-level objects (pane, tab, mux window) come straight from `mux-lua`.

use crate::app::{send_event, AppEvent, UI_STATE};
use config::keyassignment::KeyAssignment;
use mlua::{UserData, UserDataMethods, UserDataRef};
use mux::pane::PaneId;
use mux::window::WindowId;
use mux::Mux;
use mux_lua::{MuxPane, MuxTab, MuxWindow};
use std::rc::Rc;

#[derive(Clone)]
pub struct GuiWin {
    pub mux_window_id: WindowId,
}

impl UserData for GuiWin {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(methods: &mut M) {
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, _: ()| {
            Ok(format!("GuiWin(wezzy, mux_window_id:{})", this.mux_window_id))
        });
        methods.add_method("window_id", |_, this, _: ()| Ok(this.mux_window_id));
        methods.add_method("mux_window", |_, this, _: ()| Ok(MuxWindow(this.mux_window_id)));
        methods.add_method("active_tab", |_, this, _: ()| {
            let mux = Mux::get();
            let tab = mux
                .get_window(this.mux_window_id)
                .and_then(|w| w.get_active_tab().map(|t| t.tab_id()));
            Ok(tab.map(MuxTab))
        });
        methods.add_method("active_pane", |_, this, _: ()| {
            let mux = Mux::get();
            Ok(mux
                .get_active_tab_for_window(this.mux_window_id)
                .and_then(|t| t.get_active_pane())
                .map(|p| MuxPane(p.pane_id())))
        });
        methods.add_method("active_workspace", |_, _, _: ()| {
            Ok(Mux::get().active_workspace())
        });
        methods.add_method("active_key_table", |_, _, _: ()| {
            Ok(UI_STATE.lock().active_key_table.clone())
        });
        methods.add_method("leader_is_active", |_, _, _: ()| {
            Ok(UI_STATE.lock().leader_active)
        });
        methods.add_method("is_focused", |_, _, _: ()| Ok(true));
        methods.add_method(
            "perform_action",
            // pane is optional: without it, the active pane
            |_, _, (action, pane): (KeyAssignment, Option<UserDataRef<MuxPane>>)| {
                send_event(AppEvent::Perform {
                    pane_id: pane.map(|p| p.0),
                    action,
                });
                Ok(())
            },
        );
        // wezzy.action.SpawnCommandInNewPane (src/lua/wezzy.lua)
        methods.add_method(
            "spawn_command_in_new_pane",
            |_, _, spec: config::keyassignment::SpawnCommand| {
                send_event(AppEvent::SpawnInNewPane(spec));
                Ok(())
            },
        );
        methods.add_method("set_left_status", |_, _, s: String| {
            UI_STATE.lock().left_status = s;
            Ok(())
        });
        methods.add_method("set_right_status", |_, _, s: String| {
            UI_STATE.lock().right_status = s;
            Ok(())
        });
        methods.add_method("copy_to_clipboard", |_, _, (text, _): (String, mlua::Value)| {
            send_event(AppEvent::Clipboard(text));
            Ok(())
        });
        methods.add_method("get_selection_text_for_pane", |_, _, _: mlua::Value| {
            Ok(String::new())
        });
        methods.add_method(
            "toast_notification",
            |_, _, (title, message): (String, String)| {
                log::info!("toast: {title}: {message}");
                send_event(AppEvent::Flash(format!("{title}: {message}")));
                Ok(())
            },
        );
    }
}

/// `wezterm.on("user-var-changed", function(window, pane, name, value))`,
/// e.g. smart-splits' SMART_SPLITS_REQ from nvim.
pub fn emit_user_var_changed(mux_window_id: WindowId, pane_id: PaneId, name: String, value: String) {
    async fn run(
        lua: Option<Rc<mlua::Lua>>,
        window: GuiWin,
        pane: MuxPane,
        name: String,
        value: String,
    ) -> anyhow::Result<()> {
        let Some(lua) = lua else { return Ok(()) };
        let args = lua.pack_multi((window, pane, name, value))?;
        if let Err(err) = config::lua::emit_event(&lua, ("user-var-changed".to_string(), args)).await {
            log::error!("while processing user-var-changed: {err:#}");
        }
        send_event(AppEvent::MuxChanged);
        Ok(())
    }
    let window = GuiWin { mux_window_id };
    let pane = MuxPane(pane_id);
    promise::spawn::spawn(config::with_lua_config_on_main_thread(move |lua| {
        run(lua, window, pane, name, value)
    }))
    .detach();
}

/// Fire a `wezterm.on(name, function(window, pane, ...))` handler.
/// `extra` is passed as a third argument (PromptInputLine passes the typed line).
pub fn emit_window_event(
    name: String,
    mux_window_id: WindowId,
    pane_id: PaneId,
    extra: Option<Option<String>>,
) {
    async fn run(
        lua: Option<Rc<mlua::Lua>>,
        name: String,
        window: GuiWin,
        pane: MuxPane,
        extra: Option<Option<String>>,
    ) -> anyhow::Result<()> {
        let Some(lua) = lua else { return Ok(()) };
        let args = match extra {
            Some(line) => lua.pack_multi((window, pane, line))?,
            None => lua.pack_multi((window, pane))?,
        };
        if let Err(err) = config::lua::emit_event(&lua, (name.clone(), args)).await {
            log::error!("while processing {name} event: {err:#}");
            send_event(AppEvent::Flash(format!("lua error in {name}: {err}")));
        }
        send_event(AppEvent::MuxChanged);
        Ok(())
    }

    let window = GuiWin { mux_window_id };
    let pane = MuxPane(pane_id);
    promise::spawn::spawn(config::with_lua_config_on_main_thread(move |lua| {
        run(lua, name, window, pane, extra)
    }))
    .detach();
}
