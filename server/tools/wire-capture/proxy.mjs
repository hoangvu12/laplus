#!/usr/bin/env node
// Recording TCP proxy for the t3code reference server.
//
// Sits between a client (the unmodified web UI, or a scripted RPC client) and
// the reference server, forwards bytes untouched, and writes one NDJSON file
// per WebSocket connection describing the upgrade and every frame in both
// directions.
//
// Usage:
//   node tools/wire-capture/proxy.mjs --listen 3999 --upstream 127.0.0.1:3773 \
//        --out-dir .scratch/wire-capture --label browser
//
// Records written (one JSON object per line):
//   {"type":"connection-opened", remote}
//   {"type":"http-request",  head, method, target, headers}
//   {"type":"http-response", head, statusLine, headers}
//   {"type":"http-response-body", bodyLen, text}   (refused upgrades only)
//   {"type":"ws-frame",   dir, fin, rsv, opcode, opcodeName, masked, payloadLen}
//   {"type":"ws-message", dir, opcodeName, frames, text, json}
//   {"type":"error", side, message}                (transport failure, e.g. a reset)
//   {"type":"connection-closed", by}
//
// Frames and the messages they assemble into are both recorded: the frame
// records prove there is no framing above the WebSocket layer, the message
// records are the payloads a Rust implementation has to reproduce.

import { createServer, connect } from "node:net";
import { createWriteStream, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const OPCODE_NAMES = {
  0x0: "continuation",
  0x1: "text",
  0x2: "binary",
  0x8: "close",
  0x9: "ping",
  0xa: "pong",
};

function parseArgs(argv) {
  const args = {
    listen: 3999,
    upstreamHost: "127.0.0.1",
    upstreamPort: 3773,
    outDir: ".scratch/wire-capture",
    label: "capture",
  };
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    const value = argv[i + 1];
    switch (key) {
      case "--listen":
        args.listen = Number(value);
        break;
      case "--upstream": {
        const [host, port] = value.split(":");
        args.upstreamHost = host;
        args.upstreamPort = Number(port);
        break;
      }
      case "--out-dir":
        args.outDir = value;
        break;
      case "--label":
        args.label = value;
        break;
      default:
        throw new Error(`Unknown argument: ${key}`);
    }
  }
  return args;
}

/**
 * Pull one complete WebSocket frame off the front of `buf`.
 * Returns null when `buf` does not yet hold a whole frame.
 */
export function readFrame(buf) {
  if (buf.length < 2) return null;
  const b0 = buf[0];
  const b1 = buf[1];
  const opcode = b0 & 0x0f;
  const masked = (b1 & 0x80) !== 0;
  let payloadLen = b1 & 0x7f;
  let offset = 2;

  if (payloadLen === 126) {
    if (buf.length < offset + 2) return null;
    payloadLen = buf.readUInt16BE(offset);
    offset += 2;
  } else if (payloadLen === 127) {
    if (buf.length < offset + 8) return null;
    payloadLen = Number(buf.readBigUInt64BE(offset));
    offset += 8;
  }

  let maskKey = null;
  if (masked) {
    if (buf.length < offset + 4) return null;
    maskKey = buf.subarray(offset, offset + 4);
    offset += 4;
  }

  if (buf.length < offset + payloadLen) return null;

  let payload = buf.subarray(offset, offset + payloadLen);
  if (masked) {
    const unmasked = Buffer.from(payload);
    for (let i = 0; i < unmasked.length; i += 1) {
      unmasked[i] ^= maskKey[i % 4];
    }
    payload = unmasked;
  }

  return {
    fin: (b0 & 0x80) !== 0,
    rsv: [(b0 & 0x40) !== 0, (b0 & 0x20) !== 0, (b0 & 0x10) !== 0],
    opcode,
    masked,
    payloadLen,
    payload,
    consumed: offset + payloadLen,
  };
}

/** Split an HTTP head into its start line and header map. */
export function parseHttpHead(head) {
  const lines = head.split("\r\n").filter((line) => line.length > 0);
  const startLine = lines[0] ?? "";
  const headers = {};
  for (const line of lines.slice(1)) {
    const separator = line.indexOf(":");
    if (separator === -1) continue;
    headers[line.slice(0, separator).trim().toLowerCase()] = line.slice(separator + 1).trim();
  }
  return { startLine, headers };
}

function isWebSocketUpgrade(headers) {
  return (headers["upgrade"] ?? "").toLowerCase() === "websocket";
}

class ConnectionRecorder {
  constructor(outDir, label, index) {
    this.id = `${label}-${String(index).padStart(3, "0")}`;
    this.path = join(outDir, `${this.id}.ndjson`);
    this.stream = null;
    this.startedAt = Date.now();
    this.seq = 0;
  }

  write(record) {
    if (this.stream === null) {
      this.stream = createWriteStream(this.path, { flags: "a" });
      process.stderr.write(`[wire-capture] recording ${this.path}\n`);
    }
    this.seq += 1;
    this.stream.write(
      `${JSON.stringify({ seq: this.seq, tMs: Date.now() - this.startedAt, ...record })}\n`,
    );
  }

  close() {
    this.stream?.end();
  }
}

/**
 * Per-direction WebSocket frame decoder. Buffers bytes, emits a record per
 * frame and a record per assembled message.
 */
class DirectionDecoder {
  constructor(dir, recorder) {
    this.dir = dir;
    this.recorder = recorder;
    this.buffer = Buffer.alloc(0);
    this.pendingOpcode = null;
    this.pendingChunks = [];
    this.pendingFrames = 0;
  }

  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const frame = readFrame(this.buffer);
      if (frame === null) return;
      this.buffer = this.buffer.subarray(frame.consumed);
      this.onFrame(frame);
    }
  }

  onFrame(frame) {
    const opcodeName = OPCODE_NAMES[frame.opcode] ?? `unknown(0x${frame.opcode.toString(16)})`;
    this.recorder.write({
      type: "ws-frame",
      dir: this.dir,
      fin: frame.fin,
      rsv: frame.rsv,
      opcode: frame.opcode,
      opcodeName,
      masked: frame.masked,
      payloadLen: frame.payloadLen,
    });

    if (frame.opcode === 0x8) {
      this.recorder.write({
        type: "ws-message",
        dir: this.dir,
        opcodeName: "close",
        frames: 1,
        closeCode: frame.payloadLen >= 2 ? frame.payload.readUInt16BE(0) : null,
        closeReason: frame.payloadLen > 2 ? frame.payload.subarray(2).toString("utf8") : "",
      });
      return;
    }
    if (frame.opcode === 0x9 || frame.opcode === 0xa) {
      this.recorder.write({
        type: "ws-message",
        dir: this.dir,
        opcodeName,
        frames: 1,
        payloadHex: frame.payload.toString("hex"),
      });
      return;
    }

    if (frame.opcode === 0x1 || frame.opcode === 0x2) {
      this.pendingOpcode = frame.opcode;
      this.pendingChunks = [frame.payload];
      this.pendingFrames = 1;
    } else if (frame.opcode === 0x0) {
      this.pendingChunks.push(frame.payload);
      this.pendingFrames += 1;
    } else {
      return;
    }

    if (!frame.fin) return;

    const payload = Buffer.concat(this.pendingChunks);
    const record = {
      type: "ws-message",
      dir: this.dir,
      opcodeName: OPCODE_NAMES[this.pendingOpcode] ?? "unknown",
      frames: this.pendingFrames,
      payloadLen: payload.length,
    };
    if (this.pendingOpcode === 0x1) {
      const text = payload.toString("utf8");
      record.text = text;
      try {
        record.json = JSON.parse(text);
      } catch {
        record.json = null;
      }
    } else {
      record.payloadHex = payload.toString("hex");
    }
    this.recorder.write(record);

    this.pendingOpcode = null;
    this.pendingChunks = [];
    this.pendingFrames = 0;
  }
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  mkdirSync(args.outDir, { recursive: true });
  let connectionIndex = 0;

  const server = createServer((client) => {
    connectionIndex += 1;
    const recorder = new ConnectionRecorder(args.outDir, args.label, connectionIndex);
    const upstream = connect(args.upstreamPort, args.upstreamHost);

    // Sniff state: we only inspect the first request/response head on each TCP
    // connection, which is enough to decide whether it is the /ws upgrade.
    let clientHead = Buffer.alloc(0);
    let serverHead = Buffer.alloc(0);
    // sniff-request -> sniff-response -> frames | rejected-body | passthrough
    let phase = "sniff-request";
    let clientDecoder = null;
    let serverDecoder = null;
    // Anything the client sends between its request head and the server's 101.
    // A conforming client sends nothing here, but a fixture that silently drops
    // bytes is worse than one that records a surprise.
    let clientPending = Buffer.alloc(0);
    // A refused upgrade answers with a JSON body describing why. That body is
    // the failure shape later work has to reproduce, so it is captured rather
    // than passed through unseen.
    let rejectedBody = Buffer.alloc(0);
    let rejectedBodyLength = 0;

    let closed = false;
    const recordClose = (by) => {
      if (closed || phase === "sniff-request" || phase === "passthrough") return;
      closed = true;
      recorder.write({ type: "connection-closed", by });
    };

    const fail = (side) => (error) => {
      if (phase !== "passthrough") {
        recorder.write({ type: "error", side, message: error.message });
      }
      recordClose(side);
      recorder.close();
      client.destroy();
      upstream.destroy();
    };

    client.on("error", fail("client"));
    upstream.on("error", fail("upstream"));

    client.on("data", (chunk) => {
      upstream.write(chunk);
      if (phase === "passthrough") return;
      if (phase === "sniff-request") {
        clientHead = Buffer.concat([clientHead, chunk]);
        const end = clientHead.indexOf("\r\n\r\n");
        if (end === -1) return;
        const head = clientHead.subarray(0, end + 4).toString("utf8");
        const { startLine, headers } = parseHttpHead(head);
        if (!isWebSocketUpgrade(headers)) {
          phase = "passthrough";
          return;
        }
        const [method, target] = startLine.split(" ");
        recorder.write({
          type: "connection-opened",
          remote: `${client.remoteAddress}:${client.remotePort}`,
        });
        recorder.write({ type: "http-request", method, target, headers, head });
        phase = "sniff-response";
        clientPending = clientHead.subarray(end + 4);
        return;
      }
      if (phase === "frames") {
        clientDecoder.push(chunk);
        return;
      }
      clientPending = Buffer.concat([clientPending, chunk]);
    });

    upstream.on("data", (chunk) => {
      client.write(chunk);
      if (phase === "passthrough" || phase === "sniff-request") return;
      if (phase === "rejected-body") {
        recordRejectedBody(chunk);
        return;
      }
      if (phase === "sniff-response") {
        serverHead = Buffer.concat([serverHead, chunk]);
        const end = serverHead.indexOf("\r\n\r\n");
        if (end === -1) return;
        const head = serverHead.subarray(0, end + 4).toString("utf8");
        const { startLine, headers } = parseHttpHead(head);
        recorder.write({ type: "http-response", statusLine: startLine, headers, head });
        const remainder = serverHead.subarray(end + 4);

        if (!startLine.includes(" 101 ")) {
          phase = "rejected-body";
          rejectedBodyLength = Number(headers["content-length"] ?? 0);
          recordRejectedBody(remainder);
          return;
        }

        phase = "frames";
        clientDecoder = new DirectionDecoder("client-to-server", recorder);
        serverDecoder = new DirectionDecoder("server-to-client", recorder);
        if (remainder.length > 0) serverDecoder.push(remainder);
        if (clientPending.length > 0) {
          clientDecoder.push(clientPending);
          clientPending = Buffer.alloc(0);
        }
        return;
      }
      serverDecoder.push(chunk);
    });

    /** Accumulate a refused upgrade's body and record it once it is complete. */
    function recordRejectedBody(chunk) {
      rejectedBody = Buffer.concat([rejectedBody, chunk]);
      if (rejectedBody.length < rejectedBodyLength) return;
      recorder.write({
        type: "http-response-body",
        bodyLen: rejectedBody.length,
        text: rejectedBody.toString("utf8"),
      });
      phase = "passthrough";
    }

    client.on("end", () => {
      recordClose("client");
      upstream.end();
      recorder.close();
    });
    upstream.on("end", () => {
      recordClose("server");
      client.end();
      recorder.close();
    });
  });

  server.listen(args.listen, "127.0.0.1", () => {
    process.stderr.write(
      `[wire-capture] listening on 127.0.0.1:${args.listen} -> ${args.upstreamHost}:${args.upstreamPort}\n` +
        `[wire-capture] writing to ${args.outDir}/${args.label}-NNN.ndjson\n`,
    );
  });
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main();
}
