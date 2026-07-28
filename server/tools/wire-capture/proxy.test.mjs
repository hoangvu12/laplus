// The frame decoder is the only real logic in the capture proxy: if it is
// wrong, every fixture it writes is quietly wrong too. Exercise it against
// hand-built frames covering each length encoding, masking, and fragmentation.
//
//   node --test tools/wire-capture/proxy.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";

import { readFrame, parseHttpHead } from "./proxy.mjs";

/** Build a WebSocket frame the way a conforming peer would. */
function frame({ fin = true, opcode = 0x1, payload = "", maskKey = null }) {
  const body = Buffer.from(payload);
  const header = [fin ? 0x80 | opcode : opcode];
  const lengthBits = maskKey ? 0x80 : 0x00;

  if (body.length < 126) {
    header.push(lengthBits | body.length);
  } else if (body.length < 0x10000) {
    header.push(lengthBits | 126, (body.length >> 8) & 0xff, body.length & 0xff);
  } else {
    header.push(lengthBits | 127);
    const extended = Buffer.alloc(8);
    extended.writeBigUInt64BE(BigInt(body.length));
    header.push(...extended);
  }

  const parts = [Buffer.from(header)];
  if (maskKey) {
    parts.push(maskKey);
    const masked = Buffer.from(body);
    for (let i = 0; i < masked.length; i += 1) masked[i] ^= maskKey[i % 4];
    parts.push(masked);
  } else {
    parts.push(body);
  }
  return Buffer.concat(parts);
}

test("decodes an unmasked server text frame", () => {
  const decoded = readFrame(frame({ payload: '{"_tag":"Ack"}' }));
  assert.equal(decoded.fin, true);
  assert.equal(decoded.opcode, 0x1);
  assert.equal(decoded.masked, false);
  assert.equal(decoded.payload.toString("utf8"), '{"_tag":"Ack"}');
});

test("unmasks a client frame", () => {
  const maskKey = Buffer.from([0x11, 0x22, 0x33, 0x44]);
  const decoded = readFrame(frame({ payload: "hello world", maskKey }));
  assert.equal(decoded.masked, true);
  assert.equal(decoded.payload.toString("utf8"), "hello world");
});

test("handles the 16-bit and 64-bit length encodings", () => {
  const medium = "x".repeat(1000);
  assert.equal(readFrame(frame({ payload: medium })).payload.toString("utf8"), medium);

  const large = "y".repeat(70_000);
  const decodedLarge = readFrame(frame({ payload: large }));
  assert.equal(decodedLarge.payloadLen, 70_000);
  assert.equal(decodedLarge.payload.toString("utf8"), large);
});

test("returns null until a whole frame has arrived", () => {
  const complete = frame({ payload: "partial arrival" });
  for (let cut = 1; cut < complete.length; cut += 1) {
    assert.equal(readFrame(complete.subarray(0, cut)), null, `expected null at ${cut} bytes`);
  }
  assert.notEqual(readFrame(complete), null);
});

test("reports how many bytes a frame consumed so the next one can be read", () => {
  const first = frame({ payload: "one" });
  const second = frame({ payload: "two" });
  const stream = Buffer.concat([first, second]);

  const decodedFirst = readFrame(stream);
  assert.equal(decodedFirst.payload.toString("utf8"), "one");
  assert.equal(decodedFirst.consumed, first.length);

  const decodedSecond = readFrame(stream.subarray(decodedFirst.consumed));
  assert.equal(decodedSecond.payload.toString("utf8"), "two");
});

test("exposes fin and continuation opcodes so fragmented messages reassemble", () => {
  const start = readFrame(frame({ fin: false, opcode: 0x1, payload: "frag" }));
  assert.equal(start.fin, false);
  assert.equal(start.opcode, 0x1);

  const rest = readFrame(frame({ fin: true, opcode: 0x0, payload: "ment" }));
  assert.equal(rest.fin, true);
  assert.equal(rest.opcode, 0x0);
});

test("parses an HTTP head into its start line and lowercased headers", () => {
  const head =
    "GET /ws?wsTicket=abc HTTP/1.1\r\n" +
    "Host: 127.0.0.1:3999\r\n" +
    "Upgrade: websocket\r\n" +
    "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" +
    "\r\n";
  const { startLine, headers } = parseHttpHead(head);
  assert.equal(startLine, "GET /ws?wsTicket=abc HTTP/1.1");
  assert.equal(headers["upgrade"], "websocket");
  assert.equal(headers["sec-websocket-key"], "dGhlIHNhbXBsZSBub25jZQ==");
});
