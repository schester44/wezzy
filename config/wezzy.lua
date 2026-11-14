-- Wezzy config. App settings also live in ⌘, (settings.json); this file wins.
-- Keys are built in: Leader (⌃A) ? lists them all. Keys here override them
-- one at a time.

local wezzy = require("wezzy")

local config = wezzy.setup({
	color_scheme = "Tokyo Night",
	font = { family = { "JetBrains Mono", "Menlo" }, size = 14 },
	cursor = "BlinkingBar",
	window = {
		padding = { left = 16, right = 16, top = 12, bottom = 12 },
		opacity = 0.94,
	},
	wezterm = { enable_kitty_keyboard = true },

	-- keys = {
	--   { "cmd+b", wezzy.action.ToggleSidebar },
	-- },
})

return config
