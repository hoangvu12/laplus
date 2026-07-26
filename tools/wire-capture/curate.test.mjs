// Curation is the only thing between a live session token and a committed
// fixture, so its redaction is tested rather than eyeballed.
//
//   node --test tools/wire-capture/curate.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";

import { describeToken, credentialSubstitutions, curateRecord } from "./curate.mjs";

/** Build a token shaped like the ones the reference server issues. */
function signedToken(claims, signature = "s1gn4tur3") {
  return `${Buffer.from(JSON.stringify(claims)).toString("base64url")}.${signature}`;
}

const sessionToken = signedToken({ v: 1, kind: "session", sid: "abc", exp: 1 });
const ticketToken = signedToken({ v: 1, kind: "websocket", sid: "abc", exp: 1 });

test("describes a signed token by its claim names, never its value", () => {
  const described = describeToken(sessionToken);
  assert.match(described, /^<redacted:signed segments=2 claims=v,kind,sid,exp len=\d+>$/);
  assert.ok(!described.includes(sessionToken));
});

test("falls back to an opaque marker for tokens it cannot decode", () => {
  assert.match(describeToken("not-a-token"), /^<redacted:opaque segments=1 len=11>$/);
});

test("redacts the wsTicket query parameter and the session cookie", () => {
  const record = {
    type: "http-request",
    method: "GET",
    target: `/ws?wsTicket=${ticketToken}`,
    headers: {
      host: "127.0.0.1:3999",
      cookie: `t3_session=${sessionToken}`,
      upgrade: "websocket",
    },
    head: `GET /ws?wsTicket=${ticketToken} HTTP/1.1\r\nCookie: t3_session=${sessionToken}\r\n\r\n`,
  };

  const curated = curateRecord(record);
  const serialized = JSON.stringify(curated);
  assert.ok(!serialized.includes(ticketToken), "wsTicket survived curation");
  assert.ok(!serialized.includes(sessionToken), "session cookie survived curation");

  // The shape the permissive local handshake has to accept is still legible.
  assert.match(curated.target, /^\/ws\?wsTicket=<redacted:signed .*kind.*>$/);
  assert.match(curated.headers.cookie, /^t3_session=<redacted:signed /);
  assert.equal(curated.headers.upgrade, "websocket");
  assert.match(curated.head, /^GET \/ws\?wsTicket=<redacted:signed /);
  assert.ok(curated.head.includes("HTTP/1.1"), "the raw head is otherwise preserved");
});

test("finds every credential shape the server accepts", () => {
  const bearer = signedToken({ v: 1, kind: "bearer" });
  const substitutions = credentialSubstitutions({
    target: `/ws?wsTicket=${ticketToken}`,
    headers: {
      cookie: `t3_session=${sessionToken}`,
      authorization: `Bearer ${bearer}`,
    },
  });
  assert.deepEqual(
    substitutions.map(([secret]) => secret),
    [ticketToken, sessionToken, bearer],
  );
});

test("redacts a Set-Cookie the server sends back", () => {
  const curated = curateRecord({
    type: "http-response",
    statusLine: "HTTP/1.1 200 OK",
    headers: { "set-cookie": `t3_session=${sessionToken}; Path=/; HttpOnly` },
    head: `HTTP/1.1 200 OK\r\nSet-Cookie: t3_session=${sessionToken}; Path=/; HttpOnly\r\n\r\n`,
  });
  assert.ok(!JSON.stringify(curated).includes(sessionToken));
  assert.match(curated.headers["set-cookie"], /^t3_session=<redacted:signed .*>; Path=\/; HttpOnly$/);
});

test("leaves short cookie values alone rather than corrupting the head", () => {
  // A blind substring replace of "1" would rewrite every "HTTP/1.1" and every
  // port number in the head.
  const head = "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:3999\r\nCookie: theme=1\r\n\r\n";
  const curated = curateRecord({
    type: "http-request",
    target: "/ws",
    headers: { host: "127.0.0.1:3999", cookie: "theme=1" },
    head,
  });
  assert.equal(curated.head, head);
  assert.equal(curated.headers.host, "127.0.0.1:3999");
  assert.equal(curated.headers.cookie, "theme=1");
});

test("drops the decoded mirror of a text frame but keeps the wire text", () => {
  const curated = curateRecord({
    type: "ws-message",
    dir: "server-to-client",
    text: '{"_tag":"Pong"}',
    json: { _tag: "Pong" },
  });
  assert.equal(curated.text, '{"_tag":"Pong"}');
  assert.ok(!("json" in curated));
});

test("leaves frame records alone", () => {
  const frame = { type: "ws-frame", dir: "client-to-server", opcode: 1, payloadLen: 42 };
  assert.deepEqual(curateRecord(frame), frame);
});
