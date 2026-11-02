import { mkdirSync, renameSync, writeFileSync, unlinkSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"
import { randomUUID } from "node:crypto"

// Official OpenCode 1.19.3 TUI plugin. The TUI owns the active route and PID;
// its client reaches the same worker even when no HTTP listener exists.
export default {
  id: "tmux-agent-plane",
  async tui(api) {
    const pane = process.env.TMUX_PANE
    if (!pane) return
    const base = process.env.XDG_STATE_HOME || join(homedir(), ".local", "state")
    const directory = join(base, "tmux-agent-plane", "opencode")
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    const path = join(directory, `${process.pid}.json`)
    const generation = randomUUID()
    const active = new Set()
    let completed = 0
    let disposed = false
    let polling = false
    let revision = 0

    function write(value) {
      if (disposed) return
      const data = { version: 1, pid: process.pid, pane_id: pane, updated_at: Date.now(), completion_epoch: generation, completed_turns: completed, ...value }
      writeFileSync(`${path}.tmp`, JSON.stringify(data), { mode: 0o600 })
      renameSync(`${path}.tmp`, path)
    }

    function validateStatus(type) {
      if (!["busy", "retry", "idle"].includes(type)) throw new Error(`unknown OpenCode status ${type}`)
    }

    function record(id, type) {
      validateStatus(type)
      if (type === "idle") {
        if (active.delete(id)) completed++
        return
      }
      // Background subagents do not complete the foreground turn. Remember only
      // work observed on this TUI's selected route, even if it later goes home.
      const route = api.route.current
      if (route.name === "session" && route.params?.sessionID === id) active.add(id)
    }

    function routeKey() {
      const route = api.route.current
      return JSON.stringify([route.name, route.params?.sessionID, api.state.path.directory])
    }

    function publish(statuses, permissions, questions, routeIdentity, verifiedAt) {
      if (routeIdentity !== routeKey()) return
      const route = api.route.current
      if (route.name !== "home" && route.name !== "session") throw new Error(`unknown OpenCode route ${route.name}`)
      const nativeID = route.name === "session" ? route.params?.sessionID : undefined
      if (route.name === "session" && typeof nativeID !== "string") throw new Error("missing active session ID")
      // These are direct children plus the root, matching OpenCode's permission UI.
      const root = nativeID && api.state.session.get(nativeID)
      if (nativeID && !root) return // Wait for session synchronization; never invent its state.
      const family = new Set(nativeID ? [nativeID] : [])
      if (root && !root.parentID) {
        for (const request of [...permissions, ...questions]) {
          if (api.state.session.get(request.sessionID)?.parentID === nativeID) family.add(request.sessionID)
        }
      }
      for (const status of Object.values(statuses)) validateStatus(status.type)
      // SessionStatus.set(idle) removes the map entry. Absence in a successful
      // native snapshot means idle, per src/session/status.ts; never use history.
      const type = nativeID ? (statuses[nativeID]?.type ?? "idle") : "idle"
      const permission = permissions.find((item) => family.has(item.sessionID))
      const question = questions.find((item) => family.has(item.sessionID))
      const state = permission || question ? "blocked" : type === "idle" ? "idle" : "working"
      write({
        updated_at: verifiedAt,
        session_id: nativeID ?? `${generation}:home`,
        state,
        detail: permission ? `permission: ${permission.permission}` : question ? `question: ${question.questions?.[0]?.question ?? "input needed"}` : null,
        title: root?.title ?? "OpenCode",
      })
    }

    // Only this ordered stream mutates completion lifecycle. A native idle
    // snapshot can arrive before older queued busy/idle events; counting both
    // would complete the same turn twice. Snapshots supply state and freshness.
    const off = api.event.on("session.status", (event) => {
      revision++
      const { sessionID, status } = event.properties
      try {
        record(sessionID, status.type)
      } catch (error) {
        write({ session_id: `${generation}:error`, title: "OpenCode", state: "error", detail: String(error) })
      }
    })

    async function poll() {
      if (disposed || polling || !api.state.ready) return
      polling = true
      const before = revision
      const routeIdentity = routeKey()
      try {
        const options = { throwOnError: true }
        const [statuses, permissions, questions] = await Promise.all([
          api.client.session.status({}, options),
          api.client.permission.list({}, options),
          api.client.question.list({}, options),
        ])
        if (!statuses.data || typeof statuses.data !== "object" || Array.isArray(statuses.data)
            || !Array.isArray(permissions.data) || !Array.isArray(questions.data)
            || permissions.data.some((item) => typeof item.sessionID !== "string" || typeof item.permission !== "string")
            || questions.data.some((item) => typeof item.sessionID !== "string" || !Array.isArray(item.questions)
              || item.questions.some((question) => typeof question.question !== "string"))) {
          throw new Error("invalid native OpenCode snapshot")
        }
        // An event arriving during these requests supersedes this snapshot.
        if (before !== revision || routeIdentity !== routeKey()) return
        // Publish the current event counter atomically with this verified state.
        // Publishing it from an idle event against an older busy snapshot could
        // make the daemon consume a completion before it can derive done.
        publish(statuses.data, permissions.data, questions.data, routeIdentity, Date.now())
      } catch (error) {
        write({ session_id: `${generation}:error`, title: "OpenCode", state: "error", detail: String(error) })
      } finally {
        polling = false
      }
    }
    const timer = setInterval(() => { void poll() }, 250)
    api.lifecycle.onDispose(() => {
      disposed = true
      clearInterval(timer)
      off()
      try { unlinkSync(path) } catch (error) { if (error.code !== "ENOENT") throw error }
    })
    await poll()
  },
}
