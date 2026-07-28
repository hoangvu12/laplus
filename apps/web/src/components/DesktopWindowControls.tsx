import { useEffect, useState } from "react";

import { invokeWindowCommand, isDesktopShell, isWindowMaximized } from "../desktopShell";

/**
 * Minimise, maximise and close, drawn by the web app.
 *
 * Upstream never draws these. On Windows it asks Electron for
 * `titleBarOverlay`, and the *operating system* draws the buttons over the
 * page; all the UI does is keep out of their way, through `navigator
 * .windowControlsOverlay` and the `env(titlebar-area-*)` insets behind the
 * `.wco` class. WebView2 exposes no such API, so that path cannot be switched
 * on here however the shell is configured — the buttons have to be a component.
 *
 * The width is upstream's, so the two shells have caption buttons the same
 * size; the height is the topbar's rather than upstream's, which is the one
 * place this deliberately looks better than what it copies.
 * `--desktop-window-control-width` in `index.css` carries both numbers and the
 * argument, and the topbar reads them back to reserve the space.
 *
 * Everything else is Windows 11's own: a transparent rest state, a faint hover,
 * and a red close button — a window whose controls do not look like the
 * platform's reads as a web page pretending, which is the thing ticket 27 is
 * about.
 */
export function DesktopWindowControls() {
  if (!isDesktopShell) {
    return null;
  }
  return <WindowControls />;
}

function WindowControls() {
  const maximized = useMaximized();

  return (
    <div
      className="fixed top-0 right-0 z-[1000] flex"
      data-desktop-window-controls
      // The one region of the window that must never drag it: these are the
      // controls. Tauri's drag script already stops at a <button>, and this
      // says so at the container as well, so a future non-button child cannot
      // quietly become a drag handle.
      data-tauri-drag-region="false"
    >
      <WindowControlButton label="Minimise" onClick={() => void invokeWindowCommand("minimize")}>
        <path d="M2.5 8h11" />
      </WindowControlButton>
      <WindowControlButton
        label={maximized ? "Restore" : "Maximise"}
        onClick={() => void invokeWindowCommand("toggle_maximize")}
      >
        {maximized ? (
          <>
            <path d="M4.5 6.5h7v7h-7z" />
            <path d="M6.5 4.5h7v7h-2" />
          </>
        ) : (
          <path d="M4.5 4.5h9v9h-9z" />
        )}
      </WindowControlButton>
      <WindowControlButton
        label="Close"
        onClick={() => void invokeWindowCommand("close")}
        destructive
      >
        <path d="M4.75 4.75l8.5 8.5m0-8.5l-8.5 8.5" />
      </WindowControlButton>
    </div>
  );
}

function WindowControlButton({
  label,
  onClick,
  destructive = false,
  children,
}: {
  readonly label: string;
  readonly onClick: () => void;
  readonly destructive?: boolean;
  readonly children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={
        destructive
          ? "desktop-window-control desktop-window-control-close"
          : "desktop-window-control"
      }
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 18 18"
        fill="none"
        stroke="currentColor"
        strokeWidth="1"
        shapeRendering="crispEdges"
        aria-hidden="true"
      >
        {children}
      </svg>
    </button>
  );
}

/**
 * Whether the window is maximised, which is the only thing the middle button
 * has to know and the only thing this file asks the shell for.
 *
 * Polled off the DOM's own `resize` rather than Tauri's `tauri://resize`
 * event: a window cannot change between maximised and restored without its
 * webview changing size, the listener costs nothing, and it keeps the
 * capability down to the four commands the buttons actually press — no event
 * permissions, no subscription to unwind.
 */
function useMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let live = true;
    const sync = () => {
      void isWindowMaximized().then((value) => {
        if (live) {
          setMaximized(value);
        }
      });
    };

    sync();
    window.addEventListener("resize", sync);
    return () => {
      live = false;
      window.removeEventListener("resize", sync);
    };
  }, []);

  return maximized;
}
