import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import "./styles.css";

// The page is a browser underneath, and a right-click opened the browser's own
// menu: Back, Reload, Print, Inspect. None of it belongs on the notch, and
// Reload is one stray click from a game away from resetting it. Text fields
// keep theirs, since copy and paste are what that menu is for; development
// builds keep it everywhere, for Inspect.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (event) => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable='true']")) return;
    event.preventDefault();
  });
}

const root = document.getElementById("root");
if (!root) throw new Error("#root is missing from index.html");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
