#!/usr/bin/env node
// Scripted RPC client for the t3code reference server.
//
// The browser exercises the happy path on its own, but a typed error and a
// cleanly-terminated stream need deliberate driving. This client speaks the
// same socket the UI speaks, through the recording proxy, so the frames land
// in the same fixture format.
//
// Usage:
//   node tools/wire-capture/scripted-client.mjs --scenario stream \
//        --base http://127.0.0.1:3999 --token <bearer>
//
// Scenarios:
//   unary   a successful request/response pair
//   error   a request that fails with a typed error, and an unknown method tag
//   stream  a subscription from first chunk through client-initiated interrupt

import { tmpdir } from "node:os";
import { join } from "node:path";

const args = Object.fromEntries(
  process.argv
    .slice(2)
    .reduce(
      (pairs, value, index, all) =>
        index % 2 === 0 ? [...pairs, [value.slice(2), all[index + 1]]] : pairs,
      [],
    ),
);

const base = args.base ?? "http://127.0.0.1:3999";
const scenario = args.scenario ?? "unary";
const token = args.token;
if (!token) throw new Error("--token <bearer> is required");

/** Exchange the bearer token for the short-lived ticket the socket URL carries. */
async function issueWebSocketTicket() {
  const response = await fetch(new URL("/api/auth/websocket-ticket", base), {
    method: "POST",
    headers: { authorization: `Bearer ${token}` },
  });
  if (!response.ok) {
    throw new Error(`websocket-ticket failed: ${response.status} ${await response.text()}`);
  }
  return (await response.json()).ticket;
}

function log(direction, message) {
  const text = typeof message === "string" ? message : JSON.stringify(message);
  process.stdout.write(`${direction} ${text.length > 300 ? `${text.slice(0, 300)}…` : text}\n`);
}

/**
 * Open the socket and run `script`, which receives helpers for sending
 * envelopes and awaiting server messages that match a predicate.
 */
async function drive(script) {
  const ticket = await issueWebSocketTicket();
  const socketUrl = new URL("/ws", base);
  socketUrl.protocol = socketUrl.protocol === "https:" ? "wss:" : "ws:";
  socketUrl.searchParams.set("wsTicket", ticket);

  const socket = new WebSocket(socketUrl);
  const received = [];
  const waiters = [];

  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    log("S>C", message);
    received.push(message);
    for (const waiter of [...waiters]) {
      if (waiter.predicate(message)) {
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(message);
      }
    }
  });

  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", () => reject(new Error("socket error")), { once: true });
  });

  const send = (message) => {
    log("C>S", message);
    socket.send(JSON.stringify(message));
  };
  const waitFor = (predicate, timeoutMs = 10_000) =>
    new Promise((resolve, reject) => {
      const existing = received.find(predicate);
      if (existing) return resolve(existing);
      const waiter = { predicate, resolve };
      waiters.push(waiter);
      setTimeout(() => {
        if (waiters.includes(waiter)) {
          waiters.splice(waiters.indexOf(waiter), 1);
          reject(new Error(`timed out waiting for a matching message after ${timeoutMs}ms`));
        }
      }, timeoutMs).unref?.();
    });

  const request = (id, tag, payload) => send({ _tag: "Request", id, tag, payload, headers: [] });

  try {
    await script({ send, request, waitFor, received });
  } finally {
    socket.close(1000, "capture complete");
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
}

const isExit = (id) => (message) => message._tag === "Exit" && message.requestId === id;
const isChunk = (id) => (message) => message._tag === "Chunk" && message.requestId === id;

const scenarios = {
  // A plain request/response pair, plus the keepalive exchange.
  async unary({ send, request, waitFor }) {
    request("0", "server.getConfig", {});
    await waitFor(isExit("0"));
    send({ _tag: "Ping" });
    await waitFor((message) => message._tag === "Pong");
  },

  // Two failure shapes: a declared typed error from a real method, and a tag
  // the server has no handler for.
  async error({ request, waitFor }) {
    request("0", "projects.readFile", {
      cwd: "C:\\laplus-wire-capture-does-not-exist",
      relativePath: "missing.txt",
    });
    await waitFor(isExit("0"));

    request("1", "no.such.method", {});
    await waitFor(
      (message) =>
        (message._tag === "Exit" && message.requestId === "1") ||
        message._tag === "ClientProtocolError" ||
        message._tag === "Defect",
    );
  },

  // Subscribe, acknowledge the first batch, then interrupt and watch the
  // stream terminate.
  async stream({ send, request, waitFor }) {
    request("0", "subscribeTerminalMetadata", {});
    const first = await waitFor(isChunk("0"));
    send({ _tag: "Ack", requestId: first.requestId });
    await new Promise((resolve) => setTimeout(resolve, 500));
    send({ _tag: "Interrupt", requestId: "0" });
    await waitFor(isExit("0"));
  },

  // The `stream` scenario only ever sees a snapshot. This one drives the
  // orchestration surface — the spec calls it the core — so the subscription
  // emits real deltas, and it settles whether `Ack` is genuine back-pressure by
  // withholding one and watching the stream stall.
  async orchestration({ send, request, waitFor, received }) {
    const holds = (kind) => (message) =>
      message._tag === "Chunk" &&
      message.requestId === "0" &&
      message.values.some((value) => value.kind === kind);
    const chunkCount = () => received.filter(isChunk("0")).length;

    request("0", "orchestration.subscribeShell", { requestCompletionMarker: true });
    const snapshot = await waitFor(isChunk("0"));
    send({ _tag: "Ack", requestId: snapshot.requestId });
    const synchronized = await waitFor(holds("synchronized"));

    // Deliberately leave `synchronized` unacknowledged, then cause a shell
    // change. If `Ack` is back-pressure, nothing more arrives until we send it.
    // A fresh workspace root per run: the server rejects a second project on a
    // root that already has one, so reusing a path makes the scenario
    // non-repeatable.
    const projectId = crypto.randomUUID();
    request("1", "orchestration.dispatchCommand", {
      type: "project.create",
      commandId: `wire-capture:create:${crypto.randomUUID()}`,
      projectId,
      title: "wire-capture",
      workspaceRoot: join(tmpdir(), `wire-capture-${projectId}`),
      createWorkspaceRootIfMissing: true,
      createdAt: new Date().toISOString(),
    });
    await waitFor(isExit("1"));

    const heldAt = chunkCount();
    await new Promise((resolve) => setTimeout(resolve, 2000));
    process.stdout.write(
      `[scripted-client] chunks while one Ack was withheld: ${chunkCount() - heldAt}\n`,
    );

    // Release the back-pressure; the delta the create produced now flows.
    send({ _tag: "Ack", requestId: synchronized.requestId });
    const upserted = await waitFor(holds("project-upserted"));
    send({ _tag: "Ack", requestId: upserted.requestId });

    request("2", "orchestration.dispatchCommand", {
      type: "project.delete",
      commandId: `wire-capture:delete:${crypto.randomUUID()}`,
      projectId,
    });
    await waitFor(isExit("2"));
    const removed = await waitFor(holds("project-removed"));
    send({ _tag: "Ack", requestId: removed.requestId });

    send({ _tag: "Interrupt", requestId: "0" });
    await waitFor(isExit("0"));
  },
};

const selected = scenarios[scenario];
if (!selected) {
  throw new Error(`Unknown scenario "${scenario}". Known: ${Object.keys(scenarios).join(", ")}`);
}

await drive(selected);
process.stdout.write(`[scripted-client] scenario "${scenario}" complete\n`);
