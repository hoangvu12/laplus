#!/usr/bin/env node
// Turn a raw proxy recording into a committable fixture.
//
// Two things happen here, and only these two — the frame records themselves are
// passed through untouched so a fixture stays faithful to the bytes on the wire:
//
//   1. Socket-upgrade credentials are redacted. The session cookie and the
//      `wsTicket` query parameter are signed tokens; their *shape* is what
//      later work needs, not their value, so each is replaced by a marker that
//      records the token's structure.
//   2. The decoded `json` mirror of each text frame is dropped. `text` is the
//      payload as it crossed the wire and is the thing to conform to; keeping a
//      parsed copy alongside it doubles the file and invites the two to
//      disagree.
//
// Usage:
//   node tools/wire-capture/curate.mjs <raw.ndjson> <fixture.ndjson>

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Describe a signed token without revealing it, so the fixture still documents
 * what the server accepts at upgrade. The server's tokens are
 * `base64url(claims).base64url(signature)` rather than three-segment JWTs, so
 * every leading segment is tried and the first that decodes to a JSON object
 * supplies the claim names.
 */
export function describeToken(token) {
  const segments = token.split(".");
  for (const segment of segments.slice(0, -1)) {
    try {
      const decoded = JSON.parse(Buffer.from(segment, "base64url").toString("utf8"));
      if (decoded && typeof decoded === "object") {
        return `<redacted:signed segments=${segments.length} claims=${Object.keys(decoded).join(",")} len=${token.length}>`;
      }
    } catch {
      // Not this segment; keep looking.
    }
  }
  return `<redacted:opaque segments=${segments.length} len=${token.length}>`;
}

// Substitution is a blind find-and-replace across the whole head, which is what
// keeps the rest of it verbatim — but it means a short value would rewrite
// unrelated text ("1" would hit every "HTTP/1.1"). Only values long enough to be
// a credential are substituted; anything shorter is not one.
const MINIMUM_CREDENTIAL_LENGTH = 16;

/** Strip a scheme prefix like `Bearer ` / `DPoP ` from an authorization header. */
function authorizationToken(value) {
  const space = value.indexOf(" ");
  return space === -1 ? value : value.slice(space + 1);
}

/**
 * Every credential a record can carry, as [secret, marker] pairs. Substituting
 * these leaves the rest of the head verbatim.
 *
 * Covers all three shapes the reference server accepts at upgrade — the
 * `wsTicket` query parameter, the session cookie, and an `Authorization`
 * header — plus any `Set-Cookie` the server sends back, so a capture taken with
 * a different client than the browser is redacted too.
 */
export function credentialSubstitutions(record) {
  const candidates = [];

  const ticket = /[?&]wsTicket=([^&\s]+)/.exec(record.target ?? "");
  if (ticket) candidates.push(ticket[1]);

  const headers = record.headers ?? {};
  for (const pair of (headers.cookie ?? "").split(";")) {
    const separator = pair.indexOf("=");
    if (separator !== -1) candidates.push(pair.slice(separator + 1).trim());
  }
  // Set-Cookie's value is `name=token; Path=/; HttpOnly` — only the first pair
  // is the credential.
  const setCookie = headers["set-cookie"];
  if (setCookie) {
    const [assignment] = setCookie.split(";");
    const separator = assignment.indexOf("=");
    if (separator !== -1) candidates.push(assignment.slice(separator + 1).trim());
  }
  if (headers.authorization) candidates.push(authorizationToken(headers.authorization));

  return candidates
    .filter((value) => value.length >= MINIMUM_CREDENTIAL_LENGTH)
    .map((value) => [value, describeToken(value)]);
}

const applyAll = (text, substitutions) =>
  substitutions.reduce((current, [secret, marker]) => current.split(secret).join(marker), text);

export function curateRecord(record) {
  if (record.type === "http-request" || record.type === "http-response") {
    const substitutions = credentialSubstitutions(record);
    if (substitutions.length === 0) return record;
    const headers = Object.fromEntries(
      Object.entries(record.headers ?? {}).map(([name, value]) => [
        name,
        applyAll(value, substitutions),
      ]),
    );
    return {
      ...record,
      ...(record.target === undefined ? {} : { target: applyAll(record.target, substitutions) }),
      headers,
      head: applyAll(record.head ?? "", substitutions),
    };
  }
  if (record.type === "ws-message" && "json" in record) {
    const { json, ...rest } = record;
    return rest;
  }
  return record;
}

function main() {
  const [input, output] = process.argv.slice(2);
  if (!input || !output) {
    throw new Error("Usage: curate.mjs <raw.ndjson> <fixture.ndjson>");
  }

  const curated = readFileSync(input, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.stringify(curateRecord(JSON.parse(line))))
    .join("\n");

  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(output, `${curated}\n`);
  process.stdout.write(`[curate] ${input} -> ${output}\n`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  main();
}
