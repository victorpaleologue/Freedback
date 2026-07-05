import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App.jsx";
import "./styles.css";

// The shipped widget script (loaded in <head>) registers the custom elements;
// each element's connectedCallback — render() then refresh() (the first read
// against the live demo server) — fires when React attaches it to the DOM,
// i.e. during the render() call below. No mock: the widgets talk to a real
// Freedback server.
createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>
);
