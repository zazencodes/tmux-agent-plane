/** Integration fixture: real Pi agent loop, deterministic local responses, no network/usage. */
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.registerProvider("plane-fixture", {
    baseUrl: "http://127.0.0.1/unused",
    apiKey: "fixture-only",
    api: "plane-fixture-api",
    models: [{ id: "local", name: "Local integration fixture", reasoning: false,
      input: ["text"], cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 200000, maxTokens: 1000 }],
    streamSimple(model, transcript) {
      const stream = createAssistantMessageEventStream();
      const last = transcript.messages.filter((m) => m.role === "user").at(-1);
      const text = typeof last?.content === "string" ? last.content : JSON.stringify(last?.content);
      const output = { role: "assistant" as const, content: [{ type: "text" as const, text: "fixture complete" }],
        api: model.api, provider: model.provider, model: model.id,
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
        stopReason: "stop" as const, timestamp: Date.now() };
      setTimeout(() => { stream.push({ type: "done", reason: "stop", message: output }); stream.end(); }, text?.includes("slow") ? 3500 : 1);
      return stream;
    },
  });
  pi.registerCommand("plane-resume", {
    description: "Integration fixture: revisit a native session through the official API",
    handler: async (path, ctx) => { await ctx.switchSession(path.trim()); },
  });
  pi.registerCommand("plane-freeze", {
    description: "Integration fixture: freeze publisher to verify stale data rejection",
    handler: async () => { Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 6500); },
  });
  pi.registerCommand("plane-confirm", {
    description: "Integration fixture: real blocking confirmation dialog",
    handler: async (_args, ctx) => { await ctx.ui.confirm("Allow fixture command?", "No command will run."); },
  });
  pi.registerCommand("plane-question", {
    description: "Integration fixture: real blocking question dialog",
    handler: async (_args, ctx) => { await ctx.ui.input("Fixture question"); },
  });
}
