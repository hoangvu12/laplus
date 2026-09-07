// @vitest-environment happy-dom
import { act, cleanup, render } from "@testing-library/react";
import { scopeProjectRef, scopeThreadRef } from "@t3tools/client-runtime/environment";
import type {
  EnvironmentThread,
  EnvironmentThreadShell,
} from "@t3tools/client-runtime/state/shell";
import type { EnvironmentThreadStatus } from "@t3tools/client-runtime/state/threads";
import { EnvironmentId, ProjectId, ProviderInstanceId, ThreadId } from "@t3tools/contracts";
import { Atom } from "effect/unstable/reactivity";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { DraftId, useComposerDraftStore } from "../composerDraftStore";
import {
  appAtomRegistry,
  AppAtomRegistryProvider,
  resetAppAtomRegistryForTests,
} from "../rpc/atomRegistry";
import { Route } from "../routes/_chat.$environmentId.$threadId";
import { environmentThreadDetails, environmentThreadShells } from "../state/threads";

vi.mock("./ChatView", () => ({ default: () => null }));
vi.mock("./ui/sidebar", () => ({
  SidebarInset: ({ children }: { children: ReactNode }) => children,
}));
vi.mock("../state/query", () => ({ useEnvironmentQuery: () => ({ data: null }) }));
vi.mock("@tanstack/react-router", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tanstack/react-router")>()),
  useNavigate: () => vi.fn(),
}));

const environmentId = EnvironmentId.make("draft-route-environment");
const projectId = ProjectId.make("project-1");
const threadId = ThreadId.make("reserved-thread");
const ref = scopeThreadRef(environmentId, threadId);
const draftId = DraftId.make("canonical-draft");
const ThreadRoute = Route.options.component!;
const admittedShell: EnvironmentThreadShell = {
  id: threadId,
  environmentId,
  projectId,
  title: "New thread",
  modelSelection: { instanceId: ProviderInstanceId.make("mimir_mimir"), model: "saved-model" },
  runtimeMode: "full-access",
  interactionMode: "default",
  createdAt: "2026-09-06T00:00:00.000Z",
  updatedAt: "2026-09-06T00:00:00.000Z",
  archivedAt: null,
  settledOverride: null,
  settledAt: null,
  latestTurn: null,
  branch: null,
  worktreePath: null,
  session: null,
  latestUserMessageAt: null,
  hasPendingApprovals: false,
  hasPendingUserInput: false,
  hasActionableProposedPlan: false,
};

beforeEach(() => {
  useComposerDraftStore.setState({ draftsByThreadKey: {}, draftThreadsByThreadKey: {} });
  vi.spyOn(Route, "useParams").mockReturnValue(ref);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  resetAppAtomRegistryForTests();
  useComposerDraftStore.setState({ draftsByThreadKey: {}, draftThreadsByThreadKey: {} });
});

function observeSubscriptions() {
  const shellAtom = Atom.make<EnvironmentThreadShell | null>(null);
  vi.spyOn(environmentThreadShells, "threadShellAtom").mockReturnValue(shellAtom);
  vi.spyOn(environmentThreadShells, "environmentThreadRefsAtom").mockReturnValue(Atom.make([]));
  const detailAtom = vi
    .spyOn(environmentThreadDetails, "detailAtom")
    .mockReturnValue(Atom.make<EnvironmentThread | null>(null));
  const statusAtom = vi
    .spyOn(environmentThreadDetails, "statusAtom")
    .mockReturnValue(Atom.make<EnvironmentThreadStatus>("empty"));
  return { shellAtom, detailAtom, statusAtom };
}

describe("canonical draft route admission", () => {
  it.each(["detailAtom", "statusAtom"] as const)(
    "does not request %s for a local draft until shell admission, including promotion",
    (subscription) => {
      useComposerDraftStore
        .getState()
        .setProjectDraftThreadId(scopeProjectRef(environmentId, projectId), draftId, { threadId });
      const observed = observeSubscriptions();
      render(<ThreadRoute />, { wrapper: AppAtomRegistryProvider });
      expect(observed[subscription]).not.toHaveBeenCalled();

      act(() => useComposerDraftStore.getState().markDraftThreadPromoting(draftId, ref));
      expect(observed[subscription]).not.toHaveBeenCalled();

      act(() => appAtomRegistry.set(observed.shellAtom, admittedShell));
      expect(observed.detailAtom).toHaveBeenCalledWith(ref);
      expect(observed.statusAtom).toHaveBeenCalledWith(ref);
    },
  );

  it("still requests detail and status for an unknown non-draft server thread", () => {
    const observed = observeSubscriptions();
    render(<ThreadRoute />, { wrapper: AppAtomRegistryProvider });
    expect(observed.detailAtom).toHaveBeenCalledWith(ref);
    expect(observed.statusAtom).toHaveBeenCalledWith(ref);
  });
});
