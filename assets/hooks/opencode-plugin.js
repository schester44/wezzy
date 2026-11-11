// wezzy agent status: tells Wezzy what opencode is doing, for the sidebar.
// Installed by Wezzy (Settings → Agent hooks); safe to delete.
// Only active inside Wezzy panes, so opencode in other terminals is unaffected.

import { mkdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const pane = process.env.WEZTERM_PANE ?? "";
const inWezzy = !!(process.env.WEZZY || process.env.DEKITA) && /^\d+$/.test(pane);
const dir = join(
  process.env.XDG_STATE_HOME || join(process.env.HOME ?? "", ".local/state"),
  "wezzy/agents",
);

function report(payload) {
  try {
    mkdirSync(dir, { recursive: true });
    const tmp = join(dir, `.${pane}.${process.pid}`);
    const record = { harness: "opencode", pane: Number(pane), ts: Math.floor(Date.now() / 1000), payload };
    writeFileSync(tmp, JSON.stringify(record) + "\n");
    renameSync(tmp, join(dir, `${pane}.json`));
  } catch {
    // status reporting must never get in opencode's way
  }
}

// Just enough of a tool's arguments to describe it ("Editing app.rs").
function brief(args) {
  const out = {};
  for (const k of ["filePath", "path", "command", "description", "pattern"]) {
    if (typeof args?.[k] === "string") out[k] = args[k].slice(0, 200);
  }
  return out;
}

export const WezzyStatus = async ({ directory }) => {
  if (!inWezzy) return {};
  const send = (event, extra = {}) => report({ event, cwd: directory, ...extra });
  // Subagents run in sessions of their own; the agent is done when none is busy.
  const busy = new Set();
  const settle = (id, event) => {
    busy.delete(id);
    if (busy.size === 0) send(event);
  };
  send("start");
  process.on("exit", () => send("exit"));

  return {
    event: async ({ event }) => {
      const p = event.properties ?? {};
      switch (event.type) {
        case "session.status":
          if (p.status?.type === "idle") settle(p.sessionID, "session.idle");
          else if (!busy.has(p.sessionID)) {
            busy.add(p.sessionID);
            send("session.busy");
          }
          break;
        case "session.idle":
          settle(p.sessionID, "session.idle");
          break;
        case "session.error":
          settle(p.sessionID, "session.error");
          break;
        case "permission.asked":
        case "permission.updated":
          send("permission.asked", { tool: p.permission ?? p.type, message: p.title });
          break;
        case "permission.replied":
          send("permission.replied");
          break;
      }
    },
    "tool.execute.before": async (input, output) => {
      send("tool.execute.before", { tool: input.tool, input: brief(output?.args) });
    },
    "tool.execute.after": async () => send("tool.execute.after"),
  };
};
