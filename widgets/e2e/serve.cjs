"use strict";
// Minimal, dependency-free static file server for the widgets E2E.
//
// Serves the `widgets/` directory (so `demo.html` + `freedback-widgets.js`
// resolve as on a real site) on a fixed port. Exposed as a function so the
// launcher can start it in-process, but also runnable standalone:
//   node widgets/e2e/serve.cjs [port] [rootDir]
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".cjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".jsonld": "application/ld+json; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

/** Start a static server rooted at `root`, listening on `port`. */
function startStatic(root, port) {
  const server = http.createServer((req, res) => {
    // Strip query string; default to demo.html at the root.
    let urlPath = decodeURIComponent((req.url || "/").split("?")[0]);
    if (urlPath === "/" || urlPath === "") urlPath = "/demo.html";
    // Resolve safely inside the root (no path traversal).
    const filePath = path.normalize(path.join(root, urlPath));
    if (!filePath.startsWith(path.normalize(root))) {
      res.writeHead(403).end("forbidden");
      return;
    }
    fs.readFile(filePath, (err, data) => {
      if (err) {
        res.writeHead(404).end("not found");
        return;
      }
      const type = TYPES[path.extname(filePath)] || "application/octet-stream";
      res.writeHead(200, { "content-type": type }).end(data);
    });
  });
  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
}

module.exports = { startStatic };

if (require.main === module) {
  const port = Number(process.argv[2] || 8099);
  const root = path.resolve(process.argv[3] || path.join(__dirname, ".."));
  startStatic(root, port).then(() =>
    console.log(`static server: http://127.0.0.1:${port}/ (root ${root})`)
  );
}
