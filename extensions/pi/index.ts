/** Opt-in live status publisher for Pi 1.0.2+. Loaded through Pi's extension API. */
import { mkdirSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

// Pi reloads extensions without replacing the process. Keep counters across /reload,
// /new and /resume; a new process gets a different completion epoch.
const key = Symbol.for("tmux-agent-plane.pi");
const globals = globalThis as typeof globalThis & {
  [key]?: { instance: string; completedTurns: number };
};
const lifetime = (globals[key] ??= {
  instance: `${process.pid}-${performance.timeOrigin}`,
  completedTurns: 0,
});

export default function (pi: ExtensionAPI) {
  let context: ExtensionContext | undefined;
  let timer: ReturnType<typeof setInterval> | undefined;
  let path: string | undefined;
  let prompt: string | undefined;
  let blocked: string | undefined;

  function publish() {
    if (!context || !path) return;
    const nativeSession = context.sessionManager.getSessionId();
    const data = {
      version: 1,
      pid: process.pid,
      pane_id: process.env.TMUX_PANE,
      session_id: nativeSession,
      completion_epoch: lifetime.instance,
      state: blocked !== undefined ? "blocked" : context.isIdle() ? "idle" : "working",
      detail: blocked ?? null,
      title: context.sessionManager.getSessionName() ?? prompt ?? basename(context.cwd),
      completed_turns: lifetime.completedTurns,
      updated_at: Date.now(),
    };
    // Readers always see one complete snapshot, including heartbeat and counter.
    const temporary = `${path}.tmp`;
    writeFileSync(temporary, JSON.stringify(data), { mode: 0o600 });
    renameSync(temporary, path);
  }

  pi.on("session_start", (_event, ctx) => {
    if (ctx.mode !== "tui" || !process.env.TMUX_PANE || !process.env.TMUX) return;
    context = ctx;
    const base = process.env.XDG_STATE_HOME ?? join(homedir(), ".local", "state");
    const directory = join(base, "tmux-agent-plane", "pi");
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    path = join(directory, `${process.pid}.json`);
    publish();
    timer = setInterval(publish, 1000);
    timer.unref();
  });
  pi.on("input", (event) => {
    prompt = event.text.split("\n")[0];
    publish();
  });
  pi.on("agent_start", () => publish());
  pi.on("agent_settled", () => {
    if (!context) return;
    lifetime.completedTurns += 1;
    publish();
  });
  pi.on("ui_prompt_start", (event) => {
    blocked = event.title ?? event.kind;
    publish();
  });
  pi.on("ui_prompt_end", () => {
    blocked = undefined;
    publish();
  });
  pi.on("session_shutdown", () => {
    if (timer) clearInterval(timer);
    if (path) unlinkSync(path);
    context = undefined;
    path = undefined;
  });
}
