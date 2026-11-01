// Serves the built window (dist/) for the browser tests, with the same content security policy that
// the app has in tauri.conf.json, so that a page that would be blocked in the app is blocked here.
//
//   node e2e/serve.mjs            (after `npm run build`; PORT can be set, default 4173)

import { readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";

const root = resolve("dist");
const conf = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const csp = conf.app.security.csp;
const port = Number(process.env.PORT ?? 4173);

const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
};

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  const wanted = url.pathname === "/" ? "/index.html" : decodeURIComponent(url.pathname);
  const file = normalize(join(root, wanted));
  // Nothing outside dist/ is ever served.
  if (!file.startsWith(root)) {
    res.writeHead(403).end("no");
    return;
  }
  try {
    if (!statSync(file).isFile()) throw new Error("not a file");
    res.writeHead(200, {
      "content-type": types[extname(file)] ?? "application/octet-stream",
      "content-security-policy": csp,
      "cache-control": "no-store",
    });
    res.end(readFileSync(file));
  } catch {
    res.writeHead(404, { "content-security-policy": csp }).end("not found");
  }
}).listen(port, "127.0.0.1", () => {
  console.log(`serving dist/ on http://127.0.0.1:${port} with the app's policy: ${csp}`);
});
