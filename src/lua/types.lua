---@meta wezzy
-- Types for `require("wezzy")`, for lua-language-server. Wezzy writes this
-- file when it starts (it's overwritten, don't edit it); add its directory to
-- `workspace.library` to get completion in your config.

---A WezTerm key assignment: `wezzy.action.X`, `wezterm.action.X`, or a table.
---@alias wezzy.Action table|string

---@alias wezzy.Callback fun(window: table, pane: table)

---Modifiers and the key joined with `+`: `"cmd+shift+a"`, `"leader+s"`.
---Modifiers: `cmd`, `ctrl`, `alt` (`opt`), `shift`, `leader`.
---@alias wezzy.Key string

---@class wezzy.SpawnCommand
---@field args? string[] The command and its arguments; your shell when omitted.
---@field cwd? string The directory to start in.
---@field set_environment_variables? table<string, string>
---@field label? string
---@field domain? "CurrentPaneDomain"|"DefaultDomain"|{ DomainName: string }

---@class wezzy.Actions
---A command in a new pane of the current tab, placed like a new terminal.
---@field SpawnCommandInNewPane fun(spec?: wezzy.SpawnCommand): wezzy.Action
--@ACTIONS@
---Any of WezTerm's actions (`wezterm.action.<Name>`).
---@field [string] any

---@class wezzy.WindowOptions
---@field padding? number|{ left?: number, right?: number, top?: number, bottom?: number }
---@field opacity? number
---@field background_opacity? number

---@class wezzy.FontOptions
---@field family? string|string[] A family, or a list to fall back through.
---@field size? number
---@field line_height? number
---@field cell_width? number

---@class wezzy.Recipe
---@field name string
---@field prompt string
---@field harness? string The harness to run it with; the default one when omitted.

---@class wezzy.Host
---@field name string
---@field ssh string What you'd pass ssh: `user@host`, `-p 2222 user@host`, an alias.

---@class wezzy.Database
---@field name string
---@field url string

---@class wezzy.Workspace
---@field name string
---@field cwd string

---@alias wezzy.KeyBinding
---| { [1]: wezzy.Key, [2]: wezzy.Action|wezzy.Callback }
---| { key: string, mods?: string, action: wezzy.Action|wezzy.Callback }

---@alias wezzy.Plugin
---| string A plugin's URL.
---| { [1]: string, [2]: table } A URL and its options.
---| fun(config: table)
---| { apply_to_config: fun(config: table) }

---@class wezzy.Setup
---@field window? wezzy.WindowOptions
---@field font? string|wezzy.FontOptions
---@field color_scheme? string
---@field cursor? string WezTerm's `default_cursor_style`, e.g. `"SteadyBar"`.
---@field leader? wezzy.Key|{ [1]: wezzy.Key, timeout?: integer }
---@field keys? wezzy.KeyBinding[]|table<wezzy.Key, wezzy.Action|wezzy.Callback>
---@field plugins? wezzy.Plugin[]
---@field wezterm? table|fun(config: table) Raw WezTerm options, applied last.
---@field close_confirmation? "smart"|"never"|"always"
---@field agent_layout? "bsp"|"master-stack"
---@field default_harness? string
---@field editor? string[] The editor's command; `{file}`, `{line}`, `{col}` are filled in.
---@field auto_update? boolean
---@field notifications? boolean
---@field pane_cards? boolean
---@field sidebar? ("agents"|"servers"|"actions")[]
---@field harnesses? table<string, string[]> Name to command.
---@field recipes? wezzy.Recipe|wezzy.Recipe[]
---@field hosts? wezzy.Host|wezzy.Host[]
---@field databases? wezzy.Database|wezzy.Database[]
---@field workspaces? wezzy.Workspace|wezzy.Workspace[]

---@class wezzy
---@field action wezzy.Actions
---@field wezterm table The `wezterm` module.
local M = {}

---Builds the WezTerm config. Can be called more than once; each call adds to it.
---@param opts? wezzy.Setup
---@return table config
function M.setup(opts) end

---Binds a key, anywhere in the file.
---@param key wezzy.Key
---@param action wezzy.Action|wezzy.Callback
---@return table config
function M.bind(key, action) end

---The config being built.
---@return table config
function M.config() end

---`"cmd+shift+a"` to WezTerm's `"a"`, `"CMD|SHIFT"`.
---@param key wezzy.Key
---@return string key
---@return string mods
function M.parse_key(key) end

---@param fn wezzy.Callback
---@return wezzy.Action
function M.action_callback(fn) end

---@param event string
---@param fn function
function M.on(event, fn) end

return M
