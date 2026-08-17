import { ORCHESTRATION_WS_METHODS } from "@t3tools/contracts";
import * as Stream from "effect/Stream";
import { Atom } from "effect/unstable/reactivity";

import type { EnvironmentRegistry } from "../connection/registry.ts";
import { subscribe, type EnvironmentRpcInput } from "../rpc/client.ts";
import { createEnvironmentSubscriptionAtomFamily } from "./runtime.ts";
import { applySubagentStreamItem, EMPTY_SUBAGENT_STREAM_STATE } from "./subagentStream.ts";

/**
 * One atom per open child surface — **lazily**, which is the point.
 *
 * A conversation carries only the compact index row per child, so nothing here
 * runs until a developer clicks one; and the atom family's idle TTL is what
 * releases the loaded view when the surface closes. Releasing the view does not
 * stop the server recording the child.
 */
export function createSubagentEnvironmentAtoms<R, E>(
  runtime: Atom.AtomRuntime<EnvironmentRegistry | R, E>,
) {
  return {
    stream: createEnvironmentSubscriptionAtomFamily(runtime, {
      label: "environment-data:subagent:stream",
      subscribe: (input: EnvironmentRpcInput<typeof ORCHESTRATION_WS_METHODS.subscribeSubagent>) =>
        subscribe(ORCHESTRATION_WS_METHODS.subscribeSubagent, input).pipe(
          Stream.scan(EMPTY_SUBAGENT_STREAM_STATE, applySubagentStreamItem),
        ),
    }),
  };
}

export * from "./subagentStream.ts";
