import React from "react";
import ReactDOM from "react-dom/client";
import { createHashHistory, createBrowserHistory } from "@tanstack/react-router";

import "@fontsource-variable/dm-sans/index.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@xterm/xterm/css/xterm.css";
import "./index.css";

import { isElectron } from "./env";
import { isDesktopShell } from "./desktopShell";
import { getRouter } from "./router";
import {
  syncDocumentElectronPlatformClasses,
  syncDocumentWindowControlsOverlayClass,
} from "./lib/windowControlsOverlay";
import { AppRoot } from "./AppRoot";

// Electron loads the app from a file-backed shell, so hash history avoids path resolution issues.
const history = isElectron ? createHashHistory() : createBrowserHistory();

const router = getRouter(history);

if (isElectron) {
  syncDocumentElectronPlatformClasses(navigator.platform);
  syncDocumentWindowControlsOverlayClass();
}

// laplus's own window (ticket 27). Set here rather than in React so the first
// paint already has the topbar's right-hand inset, instead of the header
// reflowing once the controls mount. `isDesktopShell` is not `isElectron` and
// is deliberately not treated as it — see `desktopShell.ts`.
if (isDesktopShell) {
  document.documentElement.classList.add("desktop-shell");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <AppRoot router={router} />
  </React.StrictMode>,
);
