import { describe, expect, it } from "vite-plus/test";
import * as Schema from "effect/Schema";

import {
  EnvironmentPublicExposureRefusal,
  EnvironmentScopeRequiredError,
  PublicExposureMutationStep,
  PublicExposureRefusalReason,
} from "./environmentHttp.ts";

const decode = Schema.decodeUnknownSync(EnvironmentPublicExposureRefusal);
const decodeScopeRefusal = Schema.decodeUnknownSync(EnvironmentScopeRequiredError);
/** A plain `{ message }` decodes as nothing here, which is the whole point. */
const decodesAsRefusal = (body: unknown): boolean => {
  try {
    decode(body);
    return true;
  } catch {
    return false;
  }
};

const refusal = (over: Record<string, unknown> = {}) => ({
  _tag: "EnvironmentPublicExposurePreconditionError",
  code: "public_exposure_refused",
  reason: "consent-required",
  message: "Confirm that laplus may use the Cloudflare account certificate.",
  completed: [],
  remaining: [],
  traceId: "trace-1",
  ...over,
});

describe("a refused public-exposure command", () => {
  /**
   * The whole point of the shape. Until it existed the Cloudflare routes
   * answered 409 and 400 with an untagged `{ message }`, which decoded as none
   * of the declared errors — so a client had a status code and nothing to
   * render. Gap 4 in `.scratch/contract-parity/ledger.md`.
   */
  it("decodes as a tagged error rather than a bare message", () => {
    expect(decodesAsRefusal({ message: "Sign in first." })).toBe(false);
    const decoded = decode(refusal());
    expect(decoded._tag).toBe("EnvironmentPublicExposurePreconditionError");
    expect(decoded.reason).toBe("consent-required");
    expect(decoded.message).toBe("Confirm that laplus may use the Cloudflare account certificate.");
  });

  /**
   * The two halves of the union are told apart by their tag alone, because that
   * is what carries the status: a precondition the developer has to satisfy is
   * a 409, a rejection is a 400.
   */
  it("separates a precondition from a rejection, which is the status", () => {
    expect(decode(refusal())._tag).toBe("EnvironmentPublicExposurePreconditionError");
    expect(
      decode(
        refusal({
          _tag: "EnvironmentPublicExposureRejectedError",
          reason: "command-failed",
          message: "cloudflared could not list the account's tunnels.",
        }),
      )._tag,
    ).toBe("EnvironmentPublicExposureRejectedError");
    // A body carrying neither tag is not a refusal, whatever else it holds —
    // which is the untagged `{ message }` shape this replaced.
    expect(decodesAsRefusal({ ...refusal(), _tag: "EnvironmentHttpConflictError" })).toBe(false);
  });

  it("decodes every reason the server can give", () => {
    for (const reason of PublicExposureRefusalReason.literals) {
      expect(decode(refusal({ reason })).reason).toBe(reason);
    }
    expect(() => decode(refusal({ reason: "because" }))).toThrow();
  });

  /**
   * Tickets 06 and 07 both require a partial failure to say what already
   * happened and what is left, so that a retry repeats nothing and the wizard
   * never claims a rollback that did not occur. A string could not carry it.
   */
  it("carries the exact mutations a partial failure completed and left", () => {
    const decoded = decode(
      refusal({
        _tag: "EnvironmentPublicExposureRejectedError",
        reason: "cleanup-required",
        completed: ["credential", "tunnel-create"],
        remaining: ["dns-route"],
      }),
    );
    expect(decoded.completed).toEqual(["credential", "tunnel-create"]);
    expect(decoded.remaining).toEqual(["dns-route"]);
    for (const step of PublicExposureMutationStep.literals) {
      expect(decode(refusal({ completed: [step] })).completed).toEqual([step]);
    }
    // `cloudflared` has no `route dns delete`; removing a record is a separate
    // Cloudflare API call, so it is its own step rather than `dns-route`'s
    // mirror. See `.scratch/cloudflare-tunnel/research.md`.
    expect(PublicExposureMutationStep.literals).toContain("dns-record-delete");
  });

  /**
   * ADR-0047: a client without the scope learns only the required scope, never
   * state. Every reason above would disclose some — whether a tunnel exists,
   * whether laplus created it, how far setup got — so the scope refusal is a
   * different error and is answered first.
   */
  it("is never how a missing scope is refused", () => {
    const scoped = {
      _tag: "EnvironmentScopeRequiredError",
      code: "insufficient_scope",
      requiredScope: "access:write",
      traceId: "trace-2",
    };
    expect(() => decode(scoped)).toThrow();
    // Everything a denied client is given, read off the decoded value rather
    // than off the literal above — the schema is what decides the shape.
    const decoded = decodeScopeRefusal(scoped);
    expect(decoded.requiredScope).toBe("access:write");
    expect(Object.keys(decoded).toSorted()).toEqual(["_tag", "code", "requiredScope", "traceId"]);
    // Own keys, not `toHaveProperty`: these are `Error` subclasses, so an
    // inherited empty `message` is always there and says nothing.
    expect(decoded).not.toHaveProperty("reason");
    expect(Object.keys(decoded)).not.toContain("message");
  });
});
