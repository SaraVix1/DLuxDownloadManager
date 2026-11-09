// Thin client for dlux's REST API. Every call rejects with an Error carrying the server's
// message ({"error": "…"}), and `status` for callers that care (401 = sign in).

async function call(method, path, body) {
  const init = { method, credentials: "same-origin", headers: {} };
  if (body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const res = await fetch(path, init);
  let data = null;
  try {
    data = await res.json();
  } catch {
    // Empty or non-JSON body.
  }
  if (!res.ok) {
    const err = new Error(data?.error || `Request failed (HTTP ${res.status})`);
    err.status = res.status;
    err.data = data;
    throw err;
  }
  return data;
}

const enc = encodeURIComponent;

export const api = {
  state: () => call("GET", "/api/state"),
  check: (text) => call("POST", "/api/sources/check", { text }),
  add: (text, dir, paused) => call("POST", "/api/downloads", { text, dir, paused }),
  /** Upload a .torrent / .metalink File as the raw body. */
  async upload(file, dir, paused = false) {
    const q = `name=${enc(file.name)}&paused=${paused}${dir ? `&dir=${enc(dir)}` : ""}`;
    const res = await fetch(`/api/upload?${q}`, {
      method: "POST",
      credentials: "same-origin",
      // A form-style type would be refused by the server's CSRF guard.
      headers: { "content-type": file.type && !file.type.startsWith("text/") ? file.type : "application/octet-stream" },
      body: file,
    });
    const data = await res.json().catch(() => null);
    if (!res.ok) throw new Error(data?.error || `Upload failed (HTTP ${res.status})`);
    return data;
  },
  pause: (gid) => call("POST", `/api/downloads/${enc(gid)}/pause`, {}),
  resume: (gid) => call("POST", `/api/downloads/${enc(gid)}/resume`, {}),
  remove: (gid) => call("DELETE", `/api/downloads/${enc(gid)}`),
  pauseAll: () => call("POST", "/api/pause-all", {}),
  resumeAll: () => call("POST", "/api/resume-all", {}),
  purge: () => call("POST", "/api/purge", {}),
  limits: (gid) => call("GET", gid ? `/api/downloads/${enc(gid)}/limits` : "/api/limits"),
  setLimits: (gid, download, upload) =>
    call("PUT", gid ? `/api/downloads/${enc(gid)}/limits` : "/api/limits", { download, upload }),
  probe: (url) => call("POST", "/api/video/probe", { url }),
  startVideo: (spec) => call("POST", "/api/video", spec),
  updateYtDlp: () => call("POST", "/api/ytdlp/update", {}),
};

/** Open the live-state WebSocket and keep it open (reconnecting with backoff). */
export function live({ onState, onStatus }) {
  let ws;
  let delay = 500;
  let stopped = false;
  function connect() {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(`${proto}//${location.host}/api/ws`);
    ws.onopen = () => {
      delay = 500;
      onStatus("open");
    };
    ws.onmessage = (e) => {
      try {
        onState(JSON.parse(e.data));
      } catch {
        // Ignore a malformed frame; the next one replaces it.
      }
    };
    ws.onclose = async () => {
      if (stopped) return;
      // A refused upgrade looks like a plain close; ask the REST API why.
      try {
        await api.state();
        onStatus("closed");
      } catch (e) {
        onStatus(e.status === 401 ? "unauthorized" : "closed");
        if (e.status === 401) return;
      }
      setTimeout(connect, delay);
      delay = Math.min(delay * 2, 8000);
    };
  }
  connect();
  return () => {
    stopped = true;
    ws?.close();
  };
}
