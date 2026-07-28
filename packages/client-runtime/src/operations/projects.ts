import type { CommandId, EnvironmentId, OrchestrationCommand, ProjectId } from "@t3tools/contracts";

import {
  ensureBrowseDirectoryPath,
  findProjectByPath,
  inferProjectTitleFromPath,
  isExplicitRelativeProjectPath,
  isUnsupportedWindowsProjectPath,
  resolveProjectPathForDispatch,
} from "../state/projects.ts";
import type { EnvironmentProject } from "../state/models.ts";

export function getAddProjectInitialQuery(baseDirectory: string | null | undefined): string {
  const trimmed = baseDirectory?.trim() ?? "";
  return trimmed.length === 0 ? "~/" : ensureBrowseDirectoryPath(trimmed);
}

export function resolveAddProjectPath(input: {
  readonly rawPath: string;
  readonly currentProjectCwd?: string | null;
  readonly platform: string;
}): { readonly ok: true; readonly path: string } | { readonly ok: false; readonly error: string } {
  const rawPath = input.rawPath.trim();
  if (rawPath.length === 0) {
    return { ok: false, error: "Enter a project path." };
  }
  if (isUnsupportedWindowsProjectPath(rawPath, input.platform)) {
    return { ok: false, error: "Windows-style paths are only supported on Windows environments." };
  }
  if (isExplicitRelativeProjectPath(rawPath) && !input.currentProjectCwd) {
    return { ok: false, error: "Relative paths require an active project in this environment." };
  }
  const path = resolveProjectPathForDispatch(rawPath, input.currentProjectCwd);
  return path.length === 0 ? { ok: false, error: "Enter a project path." } : { ok: true, path };
}

export function findExistingAddProject(input: {
  readonly projects: ReadonlyArray<EnvironmentProject>;
  readonly environmentId: EnvironmentId;
  readonly path: string;
}): EnvironmentProject | null {
  return (
    findProjectByPath(
      input.projects.filter((project) => project.environmentId === input.environmentId),
      input.path,
    ) ?? null
  );
}

export function buildProjectCreateCommand(input: {
  readonly commandId: CommandId;
  readonly projectId: ProjectId;
  readonly workspaceRoot: string;
  readonly createdAt: string;
}): Extract<OrchestrationCommand, { type: "project.create" }> {
  return {
    type: "project.create",
    commandId: input.commandId,
    projectId: input.projectId,
    title: inferProjectTitleFromPath(input.workspaceRoot),
    workspaceRoot: input.workspaceRoot,
    createWorkspaceRootIfMissing: true,
    defaultModelSelection: null,
    createdAt: input.createdAt,
  };
}
