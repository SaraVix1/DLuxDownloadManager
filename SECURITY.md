# Security policy

## Reporting a vulnerability

Please **don't** open a public issue. Use GitHub's private reporting instead:
[Report a vulnerability](https://github.com/SaraVix1/DLuxDownloadManager/security/advisories/new).
You'll get an answer within a few days; fixes for confirmed issues go into a patch release.

## What dlux protects

The web UI (`dlux serve`) is the main exposure. By design:

- It listens on `127.0.0.1` by default and refuses to listen elsewhere unless a web token is
  configured (`[web] token`).
- Without a token it only answers to loopback host names (against DNS rebinding), requires the
  `Origin` of browser requests and WebSockets to match (against CSRF and cross-site WebSocket
  hijacking), and refuses form-encoded bodies.
- With a token, every API request needs it (Bearer header, an HttpOnly SameSite=Strict cookie, or
  `?token=` once to set the cookie). Tokens are compared in constant time.
- The aria2 RPC secret never leaves the server. A private aria2 started by dlux gets a random secret
  passed through a 0600 file, not the command line.
- Pages are served with a strict Content-Security-Policy and may not be framed.
- yt-dlp and ffmpeg downloaded by dlux are verified against the SHA-256 checksums published with
  their releases.

Anyone who can reach the web UI with the token can add downloads into any directory dlux can write
to, so treat the token like a password and prefer HTTPS (e.g. a reverse proxy) beyond your LAN.

## Supported versions

Only the latest release gets security fixes.
