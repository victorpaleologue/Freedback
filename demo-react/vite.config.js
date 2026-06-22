import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served at https://freedback.net/demo/ — the rich React showcase is NOT the
// landing page (the lightweight static site/index.html owns the root, since it
// is the most-hit URL). `base: "/demo/"` makes Vite prefix every hashed asset
// AND every root-absolute reference in index.html (including the public-dir
// widget script `/freedback-widgets.js`) with `/demo/`, so the bundle is
// self-contained under that subpath. Links to the shared protocol artifacts
// (/ns, /profile, /widgets) stay root-absolute and resolve site-wide.
export default defineConfig({
  base: "/demo/",
  plugins: [react()],
});
