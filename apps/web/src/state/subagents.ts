import { createSubagentEnvironmentAtoms } from "@t3tools/client-runtime/state/subagents";

import { connectionAtomRuntime } from "../connection/runtime";

export const subagentEnvironment = createSubagentEnvironmentAtoms(connectionAtomRuntime);
