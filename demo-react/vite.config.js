import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Served at the custom-domain root (https://freedback.net/), NOT a project
// subpath, so `base` is '/'. The build emits hashed assets under /assets/...
// and an index.html that becomes the Pages landing page.
export default defineConfig({
  base: "/",
  plugins: [react()],
});
