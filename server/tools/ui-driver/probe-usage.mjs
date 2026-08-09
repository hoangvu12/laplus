// Drive ticket 01's real Usage tracer bullet against an isolated server with a
// known Claude transcript home. The socket frames are part of the assertion.
//
// CHROME=/path/to/chromium node tools/ui-driver/probe-usage.mjs \
//   'http://127.0.0.1:4789/#token=…' sidebar 60 \
//   'Jul 11 to Aug 9' RAW_PRIVATE_SENTINEL
//
// Use `direct` with a `/usage#token=…` URL to verify Back's root fallback.
import { consoleLog, frameLog, launch, poll } from "./cdp.mjs";
import { mkdir, writeFile } from "node:fs/promises";

const url = process.argv[2];
const mode = process.argv[3] ?? "sidebar";
const expectedTokens = process.argv[4];
const expectedRange = process.argv[5];
const privateSentinel = process.argv[6];
const evidenceDirectory = process.argv[7] ?? null;

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
    const text = await session.evaluate("return document.body?.innerText ?? null;");
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
    const tokenControl = await poll(
      async () =>
        session.evaluate(`
      const tokens = [...document.querySelectorAll("button")]
        .find((node) => node.textContent?.trim() === "TOKENS");
      if (!tokens) return false;
      tokens.click();
      return true;
    `),
      15_000,
    );
    if (!tokenControl) throw new Error(`token metric control missing at ${width}px`);
    const text = await poll(async () => {
      const body = await session.evaluate("return document.body?.innerText ?? null;");
      return body?.toLowerCase().includes("processed tokens") && body.includes(expectedTokens)
        ? body
        : null;
    }, 15_000);
    if (!text) throw new Error(`Usage report missing at ${width}px`);
    if (!text.includes(expectedRange)) throw new Error(`inclusive range missing at ${width}px`);
    for (const label of [
      "Providers",
      "Daily processed tokens",
      "Cached input",
      "Uncached input",
      "Output",
      "Cache savings",
      "Breakdown",
      "MODEL",
      "DAY",
      "Claude Code",
      "Codex",
    ]) {
      if (!text.includes(label)) throw new Error(`${label} missing at ${width}px`);
    }
    const usageReply = frames
      .toReversed()
      .map((frame) => {
        try {
          return JSON.parse(frame.text);
        } catch {
          return null;
        }
      })
      .find((frame) => Array.isArray(frame?.exit?.value?.buckets));
    if (!usageReply) throw new Error(`Usage aggregate absent from wire at ${width}px`);
    const buckets = usageReply.exit.value.buckets;
    const processed = (bucket) =>
      bucket.totals.uncachedInputTokens +
      bucket.totals.cachedInputTokens +
      bucket.totals.cacheCreationTokens +
      bucket.totals.outputTokens;
    const wireTotal = buckets.reduce((total, bucket) => total + processed(bucket), 0);
    if (`${wireTotal}` !== expectedTokens) {
      throw new Error(
        `render expectation ${expectedTokens} disagrees with wire total ${wireTotal}`,
      );
    }
    for (const provider of ["claude", "codex"]) {
      const providerTotal = buckets
        .filter((bucket) => bucket.provider === provider)
        .reduce((total, bucket) => total + processed(bucket), 0);
      if (!text.includes(`${providerTotal}`)) {
        throw new Error(`${provider} wire total ${providerTotal} missing at ${width}px`);
      }
    }
    for (const bucket of buckets) {
      if (!text.includes(bucket.model)) throw new Error(`${bucket.model} missing at ${width}px`);
    }
    const unpriced = buckets.reduce((total, bucket) => total + bucket.unpricedRecords, 0);
    if (unpriced > 0 && !text.includes(`${unpriced} usage record is unpriced.`)) {
      throw new Error(`unpriced wire coverage missing at ${width}px`);
    }
    if (evidenceDirectory) {
      await mkdir(evidenceDirectory, { recursive: true });
      const shot = await session.send("Page.captureScreenshot", {
        format: "png",
        captureBeyondViewport: true,
      });
      await writeFile(`${evidenceDirectory}/usage-${width}.png`, shot.data, "base64");
      await writeFile(`${evidenceDirectory}/usage-${width}.txt`, text);
    }
  };

  await assertReport(1280);
  await assertReport(375);

  const beforeControls = frames.filter((frame) =>
    frame.text.includes("server.getUsageSummary"),
  ).length;
  await session.evaluate(`
    [...document.querySelectorAll("button")].find((node) => node.textContent?.trim() === "7 days")?.click();
  `);
  await poll(
    async () =>
      frames.filter((frame) => frame.text.includes("server.getUsageSummary")).length >
      beforeControls,
    10_000,
  );
  await session.evaluate(`
    [...document.querySelectorAll("button")].find((node) => node.textContent?.trim() === "30 days")?.click();
    document.querySelector('button[aria-label="Refresh usage"]')?.click();
    [...document.querySelectorAll("button")].find((node) => node.textContent?.trim() === "DAY")?.click();
  `);

  const wire = frames.map((frame) => frame.text).join("\n");
  if (!wire.includes("server.getUsageSummary")) throw new Error("Usage request absent from wire");
  if (!wire.includes(expectedTokens)) throw new Error("Usage aggregate absent from wire");
  if (wire.includes(privateSentinel)) throw new Error("raw transcript content crossed the wire");
  if (evidenceDirectory) {
    await writeFile(`${evidenceDirectory}/usage-wire.json`, JSON.stringify(frames, null, 2));
  }

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

  console.log(
    JSON.stringify({ mode, desktop: true, narrow: true, privacy: true, evidenceDirectory }),
  );
} finally {
  await session.close();
}
