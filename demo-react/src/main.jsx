import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App, { TARGETS } from "./App.jsx";
import { installMockBackend, seedDemoData } from "./mock-backend.js";
import "./styles.css";

// Install the fetch interceptor and seed deterministic demo data BEFORE React
// mounts. The shipped widget script (loaded in <head>) registers the custom
// elements, but each element's connectedCallback — which runs render() then
// refresh() (the first read) — only fires when React attaches it to the DOM,
// i.e. during the render() call below. So installing + seeding here guarantees
// the mock backend answers the very first aggregate read with non-empty data.
installMockBackend();
seedDemoData(TARGETS);

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>
);
