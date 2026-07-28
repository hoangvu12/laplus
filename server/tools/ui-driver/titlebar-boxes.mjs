// Where everything in the topbar actually is, in pixels from the window's
// right-hand edge.
//
// Ticket 27's second round. "The buttons are there but they do not line up
// with the icons" is a real report and an unactionable one: three things in
// that corner are positioned by three different rules — the caption buttons are
// `position: fixed`, the panel toggles are `position: absolute` off a CSS
// variable, and the header's own trailing group is flex content inside a
// padding box. Which of them is in the wrong place cannot be seen by looking,
// and the numbers say it in one line.
//
//   node tools/ui-driver/titlebar-boxes.mjs http://127.0.0.1:4774/
//
// Runs against an ordinary Chrome, where `isDesktopShell` is false, so it puts
// the `desktop-shell` class on by hand. That is the honest thing to measure:
// the class is the whole of what the shell contributes to layout, and adding it
// here reproduces the shell's geometry exactly without needing a webview.

import { launch, poll } from "./cdp.mjs";

// `--plain` leaves the class off, which measures the same page as an ordinary
// browser tab. Worth having as one keystroke: it is the only way to tell a
// laplus regression from spacing upstream already had.
const plain = process.argv.includes("--plain");
const positional = process.argv.slice(2).filter((argument) => !argument.startsWith("--"));
const url = positional[0] ?? "http://127.0.0.1:4773/";
const width = Number(positional[1] ?? 1442);

const session = await launch({ url });

const ready = await poll(
  () => session.evaluate(`return !!document.querySelector("[data-chat-header]")`),
  30000,
);
if (!ready) {
  console.error("no chat header appeared — is a project open in this laplus?");
  await session.close();
  process.exit(1);
}

await session.send("Emulation.setDeviceMetricsOverride", {
  width,
  height: 902,
  deviceScaleFactor: 1,
  mobile: false,
});
if (!plain) {
  await session.evaluate(`document.documentElement.classList.add("desktop-shell"); return true`);
}
await new Promise((r) => setTimeout(r, 500));

const boxes = await session.evaluate(`
  const viewport = window.innerWidth;
  // Reported as distance in from the right edge, because that is the edge
  // everything in this corner is positioned against.
  const box = (label, element) => {
    if (!element) return { label, missing: true };
    const rect = element.getBoundingClientRect();
    return {
      label,
      rightEdgeAt: Math.round(viewport - rect.right),
      leftEdgeAt: Math.round(viewport - rect.left),
      width: Math.round(rect.width),
    };
  };

  const header = document.querySelector("[data-chat-header]");
  const controls = document.querySelector(".workspace-titlebar-controls");
  const caption = document.querySelector("[data-desktop-window-controls]");
  // The last thing the header lays out itself: whatever sits furthest right
  // inside it that is not the absolutely-positioned toggles.
  let trailing = null;
  if (header) {
    for (const child of header.querySelectorAll("*")) {
      if (controls && controls.contains(child)) continue;
      if (child.childElementCount > 0) continue;
      const rect = child.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) continue;
      if (!trailing || rect.right > trailing.getBoundingClientRect().right) trailing = child;
    }
  }

  return {
    viewport,
    headerPaddingRight: header ? getComputedStyle(header).paddingRight : null,
    boxes: [
      box("caption buttons", caption),
      box("panel toggles", controls),
      box("header trailing content", trailing),
      box("chat header", header),
    ],
  };
`);

console.log(
  `${plain ? "browser layout" : "desktop-shell layout"}: viewport ${boxes.viewport}px, ` +
    `header padding-right ${boxes.headerPaddingRight}`,
);
console.log("(distances are pixels in from the window's right edge)");
for (const entry of boxes.boxes) {
  if (entry.missing) {
    console.log(`  ${entry.label.padEnd(26)} — not present`);
    continue;
  }
  console.log(
    `  ${entry.label.padEnd(26)} spans ${String(entry.leftEdgeAt).padStart(4)} → ${String(
      entry.rightEdgeAt,
    ).padStart(4)}  (${entry.width}px wide)`,
  );
}

const caption = boxes.boxes.find((b) => b.label === "caption buttons");
const toggles = boxes.boxes.find((b) => b.label === "panel toggles");
const trailing = boxes.boxes.find((b) => b.label === "header trailing content");
if (!caption.missing && !toggles.missing) {
  console.log(
    `\ngap, panel toggles → caption buttons: ${toggles.rightEdgeAt - caption.leftEdgeAt}px`,
  );
}
if (!toggles.missing && !trailing.missing) {
  console.log(
    `gap, header content → panel toggles:  ${trailing.rightEdgeAt - toggles.leftEdgeAt}px`,
  );
}

await session.close();
