-- The `wezzy` Lua module: `require("wezzy")` in the config.
--
-- `wezzy.setup{...}` turns one declarative table into the WezTerm config
-- (window, font, keys, plugins, ...) and hands the app's own options
-- (workspaces, hosts, recipes, ...) to `settings::lua_setup`.
--
-- Two processes load this file with a context table:
--   * the app (`commands::register_lua`): `{ actions = {...}, settings = fn }`
--   * the session server (`session::server_config`): `{ actions = {...} }`,
--     no settings, so both build the same WezTerm config from one file.
-- `actions` maps an app command's Lua name to its event (`wezzy.<id>`).

local ctx = ...
if type(ctx) ~= "table" then
	ctx = {}
end
local wezterm = require("wezterm")

local M = {}

-- The message names the option, so no position (it would point in here).
local function fail(fmt, ...)
	error("wezzy: " .. string.format(fmt, ...), 0)
end

-- wezzy.action.<AppCommand>, else any wezterm.action.<Name>
local app_actions = {}
for name, event in pairs(ctx.actions or {}) do
	app_actions[name] = { EmitEvent = event }
end

-- Actions that take arguments. The app does the work (luawin.rs): a callback
-- hands it the arguments through the `window` object.

--- A command in a new pane of the current tab, placed like a new terminal.
--- Takes what SpawnCommandInNewTab takes; without args, your shell.
function app_actions.SpawnCommandInNewPane(spec)
	spec = spec or {}
	if type(spec) ~= "table" then
		fail("SpawnCommandInNewPane takes a table like { args = { \"htop\" } }, got %s", type(spec))
	end
	return wezterm.action_callback(function(window)
		window:spawn_command_in_new_pane(spec)
	end)
end

M.action = setmetatable({}, {
	__index = function(_, name)
		return app_actions[name] or wezterm.action[name]
	end,
})
M.action_callback = wezterm.action_callback
M.on = wezterm.on
M.wezterm = wezterm

-- Reject typos instead of silently ignoring them.
local function only(t, allowed, where)
	for k in pairs(t) do
		if not allowed[k] then
			local names = {}
			for name in pairs(allowed) do
				names[#names + 1] = name
			end
			table.sort(names)
			fail("unknown option %s.%s (expected one of: %s)", where, tostring(k), table.concat(names, ", "))
		end
	end
end

local function set(list)
	local s = {}
	for _, v in ipairs(list) do
		s[v] = true
	end
	return s
end

local MODS = {
	cmd = "CMD",
	super = "CMD",
	ctrl = "CTRL",
	control = "CTRL",
	alt = "ALT",
	opt = "ALT",
	option = "ALT",
	meta = "ALT",
	shift = "SHIFT",
	leader = "LEADER",
}

local KEYS = {
	enter = "Enter",
	["return"] = "Enter",
	tab = "Tab",
	space = " ",
	esc = "Escape",
	escape = "Escape",
	backspace = "Backspace",
	delete = "Delete",
	del = "Delete",
	left = "LeftArrow",
	right = "RightArrow",
	up = "UpArrow",
	down = "DownArrow",
	home = "Home",
	["end"] = "End",
	pageup = "PageUp",
	pagedown = "PageDown",
}

--- "cmd+shift+a" -> "a", "CMD|SHIFT". The last part is the key; "cmd++" binds +.
function M.parse_key(spec)
	if type(spec) ~= "string" or spec == "" then
		fail("a key must be a string like \"cmd+shift+a\", got %s", tostring(spec))
	end
	local parts = {}
	for p in spec:gmatch("[^+]+") do
		parts[#parts + 1] = p
	end
	if spec:sub(-1) == "+" then
		parts[#parts + 1] = "+"
	end
	local key = table.remove(parts)
	local mods = {}
	for _, m in ipairs(parts) do
		local v = MODS[m:lower()]
		if not v then
			fail("unknown modifier %q in key %q", m, spec)
		end
		mods[#mods + 1] = v
	end
	local lower = key:lower()
	key = KEYS[lower] or (lower:match("^f%d+$") and lower:upper()) or key
	return key, #mods > 0 and table.concat(mods, "|") or "NONE"
end

local function to_action(a, where)
	if type(a) == "function" then
		return wezterm.action_callback(a)
	end
	if a == nil then
		fail("%s has no action", where)
	end
	return a
end

local function key_entry(spec, action, where)
	local key, mods = M.parse_key(spec)
	return { key = key, mods = mods, action = to_action(action, where) }
end

-- The config being built. bind() and setup() both add to it, in any order.
local config
function M.config()
	if not config then
		config = wezterm.config_builder()
		config.keys = {}
	end
	return config
end

--- wezzy.bind("cmd+d", action_or_function)
function M.bind(spec, action)
	local c = M.config()
	table.insert(c.keys, key_entry(spec, action, string.format("bind(%q)", tostring(spec))))
	return c
end

local function add_keys(c, keys)
	-- a list of { "cmd+d", action } / { key=, mods=, action= }, or { ["cmd+d"] = action }
	for k, v in pairs(keys) do
		if type(k) == "string" then
			table.insert(c.keys, key_entry(k, v, string.format("keys[%q]", k)))
		elseif type(v) == "table" and v.key then
			v.action = to_action(v.action, string.format("keys[%d]", k))
			table.insert(c.keys, v)
		elseif type(v) == "table" then
			table.insert(c.keys, key_entry(v[1], v[2], string.format("keys[%d]", k)))
		else
			fail("keys[%s] must be { \"cmd+x\", action }", tostring(k))
		end
	end
end

local function apply_window(c, w)
	only(w, set({ "padding", "opacity", "background_opacity" }), "window")
	local pad = w.padding
	if type(pad) == "number" then
		pad = { left = pad, right = pad, top = pad, bottom = pad }
	end
	if pad ~= nil then
		c.window_padding = pad
	end
	local opacity = w.opacity or w.background_opacity
	if opacity ~= nil then
		c.window_background_opacity = opacity
	end
end

local function apply_font(c, f)
	if type(f) == "string" then
		f = { family = f }
	end
	only(f, set({ "family", "size", "line_height", "cell_width" }), "font")
	if type(f.family) == "table" then
		c.font = wezterm.font_with_fallback(f.family)
	elseif f.family ~= nil then
		c.font = wezterm.font(f.family)
	end
	if f.size ~= nil then
		c.font_size = f.size
	end
	if f.line_height ~= nil then
		c.line_height = f.line_height
	end
	if f.cell_width ~= nil then
		c.cell_width = f.cell_width
	end
end

local function apply_leader(c, l)
	local spec, timeout = l, nil
	if type(l) == "table" then
		spec, timeout = l[1], l.timeout
	end
	local key, mods = M.parse_key(spec)
	c.leader = { key = key, mods = mods, timeout_milliseconds = timeout or 1000 }
end

local function apply_plugins(c, plugins)
	for i, p in ipairs(plugins) do
		if type(p) == "string" then
			wezterm.plugin.require(p).apply_to_config(c)
		elseif type(p) == "function" then
			p(c)
		elseif type(p) == "table" and type(p[1]) == "string" then
			-- { "https://...", opts }
			wezterm.plugin.require(p[1]).apply_to_config(c, p[2])
		elseif type(p) == "table" and type(p.apply_to_config) == "function" then
			p.apply_to_config(c)
		else
			fail("plugins[%d] must be a URL, { url, opts }, a module or a function", i)
		end
	end
end

-- The app's options (settings.rs reads them; settings.json is the base).
local APP = set({
	"close_confirmation",
	"agent_layout",
	"default_harness",
	"editor",
	"auto_update",
	"notifications",
	"pane_cards",
	"sidebar",
	"harnesses",
	"recipes",
	"hosts",
	"databases",
	"workspaces",
})
-- lists of named entries; a single entry may be given bare
local LISTS = set({ "recipes", "hosts", "databases", "workspaces" })
-- the app options of every setup() call so far: calling it again adds to them
local app = {}

local OPTIONS = set({ "window", "font", "color_scheme", "cursor", "leader", "keys", "plugins", "wezterm" })
for k in pairs(APP) do
	OPTIONS[k] = true
end

--- wezzy.setup{ window = ..., keys = ..., workspaces = ..., ... } -> the WezTerm config.
--- Can be called more than once; each call adds to the same config.
function M.setup(opts)
	opts = opts or {}
	if type(opts) ~= "table" then
		fail("setup takes a table, got %s", type(opts))
	end
	only(opts, OPTIONS, "setup")
	local c = M.config()

	if opts.window ~= nil then
		apply_window(c, opts.window)
	end
	if opts.font ~= nil then
		apply_font(c, opts.font)
	end
	if opts.color_scheme ~= nil then
		c.color_scheme = opts.color_scheme
	end
	if opts.cursor ~= nil then
		c.default_cursor_style = opts.cursor
	end
	if opts.leader ~= nil then
		apply_leader(c, opts.leader)
	end
	-- raw WezTerm options, after the ones above so it has the last word
	if type(opts.wezterm) == "function" then
		opts.wezterm(c)
	elseif type(opts.wezterm) == "table" then
		for k, v in pairs(opts.wezterm) do
			c[k] = v
		end
	elseif opts.wezterm ~= nil then
		fail("wezterm must be a table of WezTerm options or a function(config)")
	end
	-- plugins before keys, so your keys win over a plugin's
	if opts.plugins ~= nil then
		apply_plugins(c, opts.plugins)
	end
	if opts.keys ~= nil then
		add_keys(c, opts.keys)
	end

	if ctx.settings then
		for k in pairs(APP) do
			local v = opts[k]
			if LISTS[k] and type(v) == "table" and v.name ~= nil then
				v = { v }
			end
			if v == nil then
				-- keep what an earlier setup() call set
			elseif LISTS[k] and type(v) == "table" then
				app[k] = app[k] or {}
				for _, x in ipairs(v) do
					table.insert(app[k], x)
				end
			elseif k == "harnesses" and type(v) == "table" then
				app[k] = app[k] or {}
				for name, argv in pairs(v) do
					app[k][name] = argv
				end
			else
				app[k] = v
			end
		end
		ctx.settings(app)
	end
	return c
end

return M
