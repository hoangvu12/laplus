#!/usr/bin/env node
// Turn a raw proxy recording into a committable fixture.
//
// Three things happen here, and only these three:
//
//   1. Socket-upgrade credentials are redacted. The session cookie and the
//      `wsTicket` query parameter are signed tokens; their *shape* is what
//      later work needs, not their value, so each is replaced by a marker that
//      records the token's structure.
//   2. Account email addresses are masked wherever they appear, including
//      inside a frame's payload. This repository is public and a provider's
//      `auth.email` names a person, which no amount of protocol interest
//      justifies publishing. Unlike (1) the replacement is the same width as
//      what it replaces — see `maskEmail`.
//   3. The decoded `json` mirror of each text frame is dropped. `text` is the
//      payload as it crossed the wire and is the thing to conform to; keeping a
//      parsed copy alongside it doubles the file and invites the two to
//      disagree.
//
// (2) is the one that costs something. Before it, frame records passed through
// untouched and a fixture was byte-for-byte what crossed the wire; now that
// holds for everything except an email. The width is preserved so the weaker
// property that replaced it is still worth having: `payloadLen` remains the
// frame's true byte count, and the sizes quoted in docs/socket-wire-format.md
// remain the sizes of these files.
//
// Usage:
//   node tools/wire-capture/curate.mjs <raw.ndjson> <fixture.ndjson>

import { createHash } from "node:crypto";
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

const EMAIL_PATTERN = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g;

// Addresses that already name nobody. `.invalid` and `.example` are reserved by
// RFC 2606 for exactly this, and the server mints `…@laplus.invalid` itself as a
// git author — masking those would churn the fixture without protecting anyone.
const SYNTHETIC_DOMAINS = [".invalid", ".example", "example.com", "example.org", "example.net"];

const isSynthetic = (email) => {
  const domain = email.slice(email.lastIndexOf("@") + 1).toLowerCase();
  // A leading dot is a reserved TLD and matches any domain under it; the rest
  // are whole domains, and must match as one — `notexample.com` is somebody's.
  return SYNTHETIC_DOMAINS.some((suffix) =>
    suffix.startsWith(".")
      ? domain.endsWith(suffix)
      : domain === suffix || domain.endsWith(`.${suffix}`),
  );
};

/**
 * Mask an address without changing its width.
 *
 * A credential lives in an HTTP head, where nothing counts bytes, so it can be
 * replaced by a marker of any length. An email arrives inside a `ws-message`
 * payload, and that record's `payloadLen` is the frame's byte count — a
 * replacement of a different width would silently make that field a lie and
 * would contradict the byte counts docs/socket-wire-format.md quotes for these
 * files. So the mask is cut to the exact width of the local part it replaces.
 *
 * The domain is kept: it is not what identifies anyone, and keeping it leaves
 * the payload realistic for anyone reading the fixture. The local part becomes
 * `redacted-` plus a digest of the whole address, which makes the mask stable
 * (one account masks identically in every fixture) and injective in practice
 * (two accounts stay two accounts). A local part too short to hold the prefix
 * is simply truncated to its own width — still a mask, just a blunter one.
 */
export function maskEmail(email) {
  const at = email.lastIndexOf("@");
  const local = email.slice(0, at);
  const digest = createHash("sha256").update(email).digest("hex");
  return `${`redacted-${digest}`.slice(0, local.length)}${email.slice(at)}`;
}

/** Mask every address in a string, leaving everything around them untouched. */
export function maskEmails(text) {
  return text.replace(EMAIL_PATTERN, (email) => (isSynthetic(email) ? email : maskEmail(email)));
}

const maskEmailsIn = (record) =>
  Object.fromEntries(
    Object.entries(record).map(([key, value]) => [
      key,
      typeof value === "string" ? maskEmails(value) : value,
    ]),
  );

export function curateRecord(record) {
  if (record.type === "http-request" || record.type === "http-response") {
    const substitutions = credentialSubstitutions(record);
    const rewrite = (value) => maskEmails(applyAll(value, substitutions));
    const headers = Object.fromEntries(
      Object.entries(record.headers ?? {}).map(([name, value]) => [name, rewrite(value)]),
    );
    // Each field is replaced only if the record had it, so curation never
    // invents a key the proxy did not record.
    return {
      ...maskEmailsIn(record),
      ...(record.headers === undefined ? {} : { headers }),
      ...(record.target === undefined ? {} : { target: rewrite(record.target) }),
      ...(record.head === undefined ? {} : { head: rewrite(record.head) }),
    };
  }
  if (record.type === "ws-message") {
    const { json, ...rest } = record;
    return maskEmailsIn(rest);
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
