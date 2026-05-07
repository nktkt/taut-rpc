// Vite entry point. Standard React 18 root mount; nothing taut-specific
// happens here — the API wiring lives in `App.tsx` so this file stays
// trivially replaceable when slotting the demo into a different bundler.

import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root element in index.html");
}

createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
