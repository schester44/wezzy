-- Wezzy config: full WezTerm config syntax (https://wezterm.org/config/files.html).
-- App settings live in ⌘, — this file is for appearance and keys.

local wezterm = require("wezterm")
local config = wezterm.config_builder()

config.color_scheme = "Tokyo Night"
config.font = wezterm.font_with_fallback({ "JetBrains Mono", "Menlo" })
config.font_size = 14
config.window_background_opacity = 0.94
config.window_padding = { left = 16, right = 16, top = 12, bottom = 12 }
config.default_cursor_style = "BlinkingBar"
config.enable_kitty_keyboard = true

-- App actions bind like any WezTerm action (the session server evaluates this
-- file too, without the wezzy module, hence the pcall):
--
-- local ok, wezzy = pcall(require, "wezzy")
-- if ok then
--   config.keys = { { key = "b", mods = "CMD", action = wezzy.action.ToggleSidebar } }
-- end

return config
