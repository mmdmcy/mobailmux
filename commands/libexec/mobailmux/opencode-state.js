import { spawnSync } from "node:child_process"

const pane = process.env.TMUX_PANE
let lastState

function currentState() {
  if (!pane) return undefined
  const result = spawnSync("tmux", ["display-message", "-p", "-t", pane, "#{@mbx_state}"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  })
  return (result.stdout ?? "").trim() || undefined
}

function setState(state) {
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

export const MobailmuxState = async () => {
  const sessions = new Map()
  const permissions = new Map()
  let hasActivity = false

  function session(id) {
    if (!sessions.has(id)) sessions.set(id, { busy: false, failed: false })
    return sessions.get(id)
  }

  function updateState() {
    if (permissions.size > 0) {
      setState("WAITING")
      return
    }
    if ([...sessions.values()].some(({ busy }) => busy)) {
      setState("RUNNING")
      return
    }
    if ([...sessions.values()].some(({ failed }) => failed)) {
      setState("ERROR")
      return
    }
    setState(hasActivity ? "DONE" : "READY")
  }

  lastState = currentState()
  if (lastState !== "RUNNING" && lastState !== "WAITING") setState("READY")

  return {
    event: async ({ event }) => {
      const properties = event.properties ?? {}
      const sessionID = properties.sessionID ?? properties.session?.id ?? "current"
      const current = session(sessionID)

      if (event.type === "session.status") {
        const status = properties.status?.type ?? properties.status
        if (status === "busy" || status === "retry") {
          hasActivity = true
          current.failed = false
          current.busy = true
        } else if (status === "idle") {
          hasActivity = true
          current.busy = false
        }
        updateState()
        return
      }

      if (event.type === "session.idle") {
        hasActivity = true
        current.busy = false
      } else if (event.type === "session.error") {
        hasActivity = true
        current.busy = false
        current.failed = true
        for (const [id, owner] of permissions) {
          if (owner === sessionID) permissions.delete(id)
        }
      } else if (event.type === "permission.asked") {
        const permissionID =
          properties.id ?? properties.requestID ?? properties.permissionID ?? properties.permission?.id ?? sessionID
        permissions.set(permissionID, sessionID)
      } else if (event.type === "permission.replied") {
        const permissionID =
          properties.id ?? properties.requestID ?? properties.permissionID ?? properties.permission?.id ?? sessionID
        permissions.delete(permissionID)
      }
      updateState()
    },
  }
}
