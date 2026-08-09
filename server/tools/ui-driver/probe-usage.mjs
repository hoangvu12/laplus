// Drive ticket 01's real Usage tracer bullet against an isolated server with a
// known Claude transcript home. The socket frames are part of the assertion.
//
// CHROME=/path/to/chromium node tools/ui-driver/probe-usage.mjs \
//   'http://127.0.0.1:4789/#token=…' sidebar 60 \
//   'Jul 11, 2026 to Aug 9, 2026' RAW_PRIVATE_SENTINEL
//
// Use `direct` with a `/usage#token=…` URL to verify Back's root fallback.
import { consoleLog, frameLog, launch, poll } from "./cdp.mjs";

const url = process.argv[2];
const mode = process.argv[3] ?? "sidebar";
const expectedTokens = process.argv[4];
const expectedRange = process.argv[5];
const privateSentinel = process.argv[6];

if (!url || !expectedTokens || !expectedRange || !privateSentinel) {
  throw new Error(
    "expected URL, mode, processed-token total, inclusive range, and privacy sentinel",
  );
}

const session = await launch({ url });
const frames = frameLog(session);
const logs = consoleLog(session);

try {
  const ready = await poll(async () => {
    const text = await session.evaluate("return document.body.innerText;");
    return text?.includes("Usage") ? text : null;
  }, 15_000);
  if (!ready) throw new Error(`UI did not boot: ${logs.join("\n")}`);

  if (mode === "sidebar") {
    await session.evaluate(`
      const button = [...document.querySelectorAll("button")]
        .find((node) => node.textContent?.trim() === "Usage");
      if (!button) throw new Error("Usage sidebar button missing");
      button.click();
    `);
    const navigated = await poll(
      async () => (await session.evaluate("return location.pathname;")) === "/usage",
      10_000,
    );
    if (!navigated) throw new Error("sidebar navigation did not reach /usage");
  }

  const assertReport = async (width) => {
    await session.send("Emulation.setDeviceMetricsOverride", {
      width,
      height: 900,
      deviceScaleFactor: 1,
      mobile: width < 600,
    });
    const text = await poll(async () => {
      const body = await session.evaluate("return document.body.innerText;");
      return body?.toLowerCase().includes("processed tokens") && body.includes(expectedTokens)
        ? body
        : null;
    }, 15_000);
    if (!text) throw new Error(`Usage report missing at ${width}px`);
    if (!text.includes(expectedRange)) throw new Error(`inclusive range missing at ${width}px`);
  };

  await assertReport(1280);
  await assertReport(375);

  const wire = frames.map((frame) => frame.text).join("\n");
  if (!wire.includes("server.getUsageSummary")) throw new Error("Usage request absent from wire");
  if (!wire.includes(expectedTokens)) throw new Error("Usage aggregate absent from wire");
  if (wire.includes(privateSentinel)) throw new Error("raw transcript content crossed the wire");

  if (mode === "direct") {
    await session.evaluate(`
      const back = document.querySelector('button[aria-label="Back"]');
      if (!back) throw new Error("Back missing");
      back.click();
    `);
    const fallback = await poll(
      async () => (await session.evaluate("return location.pathname;")) === "/",
      10_000,
    );
    if (!fallback) throw new Error("Back fallback did not reach root");
  }

  console.log(JSON.stringify({ mode, desktop: true, narrow: true, privacy: true }));
} finally {
  await session.close();
}
