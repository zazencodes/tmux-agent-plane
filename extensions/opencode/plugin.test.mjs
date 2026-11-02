import { test } from "node:test"
import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, rmSync } from "node:fs"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import plugin from "./tmux-agent-plane.js"

const delay = () => new Promise((resolve) => setTimeout(resolve, 300))

test("official TUI publisher preserves identity, native state and durable completions", async (t) => {
  const root = fileURLToPath(new URL("../../target/", import.meta.url))
  const dir = mkdtempSync(join(root, "opencode-plugin-test-"))
  const original = { state: process.env.XDG_STATE_HOME, pane: process.env.TMUX_PANE }
  process.env.XDG_STATE_HOME = dir
  process.env.TMUX_PANE = "%fixture"
  let dispose
  let handler
  let statuses = {}
  let permissions = []
  let questions = []
  let failure = false
  const sessions = { ses_one: { title: "One" }, ses_two: { title: "Two" }, ses_child: { parentID: "ses_one" } }
  const api = {
    route: { current: { name: "session", params: { sessionID: "ses_one" } } },
    state: { ready: true, path: { directory: "/fixture" }, session: { get: (id) => sessions[id] } },
    client: {
      session: { status: async () => { if (failure) throw new Error("disconnected"); return { data: statuses } } },
      permission: { list: async () => ({ data: permissions }) },
      question: { list: async () => ({ data: questions }) },
    },
    event: { on: (type, cb) => { assert.equal(type, "session.status"); handler = cb; return () => { handler = undefined } } },
    lifecycle: { onDispose: (cb) => { dispose = cb } },
  }
  const path = join(dir, "tmux-agent-plane", "opencode", `${process.pid}.json`)
  const read = () => JSON.parse(readFileSync(path, "utf8"))
  const event = (id, type) => handler({ properties: { sessionID: id, status: { type } } })
  try {
    await plugin.tui(api)
    const identity = read().session_id
    const epoch = read().completion_epoch
    assert.equal(read().state, "idle")
    assert.equal(read().pid, process.pid)
    assert.equal(read().pane_id, "%fixture")

    await t.test("a sub-tick native busy/idle transition is durable and counts once", async () => {
      event("ses_one", "busy")
      assert.equal(read().completed_turns, 0)
      event("ses_one", "idle")
      event("ses_one", "idle")
      assert.equal(read().completed_turns, 1)
      await delay()
      assert.equal(read().completed_turns, 1)
    })
    await t.test("busy and idle snapshots do not prime a completion without live events", async () => {
      statuses = { ses_one: { type: "busy" } }
      await delay()
      assert.equal(read().state, "working")
      assert.equal(read().completed_turns, 1)
      statuses = {}
      await delay()
      assert.equal(read().state, "idle")
      assert.equal(read().completed_turns, 1)
    })
    await t.test("permission and question snapshots override busy and include child requests", async () => {
      statuses = { ses_one: { type: "busy" } }
      event("ses_one", "busy")
      permissions = [{ sessionID: "ses_child", permission: "bash" }]
      await delay()
      assert.equal(read().state, "blocked")
      assert.equal(read().detail, "permission: bash")
      permissions = []
      questions = [{ sessionID: "ses_one", questions: [{ question: "Choose?" }] }]
      await delay()
      assert.equal(read().detail, "question: Choose?")
      questions = []
      statuses = { ses_one: { type: "retry" } }
      await delay()
      assert.equal(read().state, "working")
      statuses = {}
      event("ses_one", "idle")
      await delay()
      assert.equal(read().state, "idle")
      assert.equal(read().completed_turns, 2)
    })
    await t.test("home, another session and revisits preserve one launch counter", async () => {
      api.route.current.params.sessionID = "ses_two"
      await delay()
      assert.notEqual(read().session_id, identity)
      assert.equal(read().completed_turns, 2)
      assert.equal(read().completion_epoch, epoch)
      api.route.current = { name: "home" }
      await delay()
      assert.equal(read().completed_turns, 2)
      assert.equal(read().completion_epoch, epoch)
      api.route.current = { name: "session", params: { sessionID: "ses_one" } }
      await delay()
      assert.equal(read().session_id, identity)
      assert.equal(read().completed_turns, 2)
      assert.equal(read().completion_epoch, epoch)
      event("ses_child", "busy")
      event("ses_child", "idle")
      assert.equal(read().completed_turns, 2)
    })
    await t.test("idle snapshot before queued busy/idle events cannot double count", async () => {
      statuses = {}
      event("ses_one", "busy")
      await delay() // Native snapshot is already idle, but queued events lag it.
      assert.equal(read().state, "idle")
      assert.equal(read().completed_turns, 2)
      event("ses_one", "busy") // Another busy event for the same turn.
      event("ses_one", "idle")
      assert.equal(read().completed_turns, 3)
      await delay()
      event("ses_one", "idle")
      assert.equal(read().completed_turns, 3)
    })
    await t.test("unknown native status and failed fresh snapshot publish errors", async () => {
      statuses = { ses_one: { type: "sleeping" } }
      await delay()
      assert.equal(read().state, "error")
      assert.match(read().detail, /unknown OpenCode status/)
      statuses = {}
      failure = true
      await delay()
      assert.equal(read().state, "error")
      assert.match(read().detail, /disconnected/)
    })
  } finally {
    dispose?.()
    for (const [key, value] of [["XDG_STATE_HOME", original.state], ["TMUX_PANE", original.pane]]) {
      if (value === undefined) delete process.env[key]
      else process.env[key] = value
    }
    rmSync(dir, { recursive: true })
  }
  assert.equal(handler, undefined)
})
