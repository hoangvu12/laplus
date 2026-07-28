import React from "react";
import ReactDOM from "react-dom/client";
import { createBrowserHistory } from "@tanstack/react-router";

import "@fontsource-variable/dm-sans/index.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@xterm/xterm/css/xterm.css";
import "./index.css";

import { isDesktopShell } from "./desktopShell";
import { getRouter } from "./router";
import { AppRoot } from "./AppRoot";

// laplus is served over http://127.0.0.1 (ADR-0010) rather than from a file,
// so ordinary path history works and there is no reason for hash history.
const router = getRouter(createBrowserHistory());

// laplus's own window (ticket 27). Set here rather than in React so the first
// paint already has the topbar's right-hand inset, instead of the header
// reflowing once the controls mount.
if (isDesktopShell) {
  document.documentElement.classList.add("desktop-shell");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <AppRoot router={router} />
  </React.StrictMode>,
);
