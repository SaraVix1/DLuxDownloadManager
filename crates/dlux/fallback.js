const fmt = b => { const u = ["B","KiB","MiB","GiB"]; let i = 0;
  while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return (i ? b.toFixed(1) : b) + " " + u[i] + "/s"; };
function connect() {
  const ws = new WebSocket((location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/api/ws");
  ws.onmessage = e => { const s = JSON.parse(e.data);
    document.getElementById("status").textContent = s.aria2.connected ? "aria2 " + s.aria2.version : "aria2 offline";
    document.getElementById("down").textContent = "↓ " + fmt(s.speed.download);
    document.getElementById("up").textContent = "↑ " + fmt(s.speed.upload);
    document.getElementById("count").textContent = s.downloads.length + " downloads"; };
  ws.onclose = () => { document.getElementById("status").textContent = "reconnecting…"; setTimeout(connect, 2000); };
}
connect();
