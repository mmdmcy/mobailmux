import { spawnSync } from "node:child_process"
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"

const pane = process.env.TMUX_PANE
let lastState: "READY" | "RUNNING" | "WAITING" | "DONE" | "ERROR" | undefined

function currentState(): "READY" | "RUNNING" | "WAITING" | "DONE" | "ERROR" | undefined {
  if (!pane) return undefined
  const result = spawnSync("tmux", ["display-message", "-p", "-t", pane, "#{@mbx_state}"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  })
  const value = (result.stdout ?? "").trim()
  if (value === "READY" || value === "RUNNING" || value === "WAITING" || value === "DONE" || value === "ERROR") {
    return value
  }
  return undefined
}

function setState(state: "READY" | "RUNNING" | "WAITING" | "DONE" | "ERROR") {
  if (!pane || state === lastState) return
  lastState = state
  spawnSync("tmux", ["set-option", "-w", "-t", pane, "@mbx_state", state], {
    stdio: "ignore",
  })
  spawnSync(
    "tmux",
    ["set-option", "-w", "-t", pane, "@mbx_state_at", String(Math.floor(Date.now() / 1000))],
    { stdio: "ignore" },
  )
  spawnSync("tmux", ["refresh-client", "-S"], { stdio: "ignore" })
}

export default function mobailmuxState(pi: ExtensionAPI) {
  let failed = false
  let aborted = false

  pi.on("project_trust", async () => {
    setState("WAITING")
    return { trusted: "undecided" }
  })

  pi.on("session_start", async () => {
    const state = currentState()
    if (state !== "RUNNING" && state !== "WAITING") setState("READY")
  })

  pi.on("agent_start", async () => {
    failed = false
    aborted = false
    setState("RUNNING")
  })

  pi.on("agent_end", async (event) => {
    for (let index = event.messages.length - 1; index >= 0; index -= 1) {
      const message = event.messages[index]
      if (message.role !== "assistant") continue
      failed = message.stopReason === "error"
      aborted = message.stopReason === "aborted"
      break
    }
  })

  pi.on("agent_settled", async () => {
    setState(failed ? "ERROR" : aborted ? "READY" : "DONE")
  })
}
