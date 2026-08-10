import {
  type EnvironmentId,
  type EditorId,
  type ProjectScript,
  type ResolvedKeybindingsConfig,
} from "@t3tools/contracts";
import { memo } from "react";
import { Tooltip, TooltipPopup, TooltipTrigger } from "../ui/tooltip";
import ProjectScriptsControl, {
  type NewProjectScriptInput,
  type ProjectScriptActionResult,
} from "../ProjectScriptsControl";
import { OpenInPicker } from "./OpenInPicker";
import { usePrimaryEnvironmentId } from "../../state/environments";
import { useT3ProjectFileScripts } from "~/hooks/useT3ProjectFileScripts";
import { ProjectFavicon } from "../ProjectFavicon";
import { cn } from "~/lib/utils";
import { Menu, MenuItem, MenuPopup, MenuTrigger } from "../ui/menu";
import { ArchiveIcon, CopyIcon, PencilIcon, PinIcon, PinOffIcon, Trash2Icon } from "lucide-react";
import { threadActionPolicy } from "../../threadActionPolicy";

interface ChatHeaderProps {
  activeThreadEnvironmentId: EnvironmentId;
  activeThreadTitle: string;
  activeProjectName: string | undefined;
  activeProjectCwd: string | null;
  openInCwd: string | null;
  activeProjectScripts: ReadonlyArray<ProjectScript> | undefined;
  preferredScriptId: string | null;
  keybindings: ResolvedKeybindingsConfig;
  availableEditors: ReadonlyArray<EditorId>;
  rightPanelOpen: boolean;
  pinningSupported: boolean;
  pinned: boolean;
  settled: boolean;
  snoozed: boolean;
  settlementSupported: boolean;
  snoozeSupported: boolean;
  onTogglePin: () => void;
  onThreadAction: (action: ChatHeaderThreadAction) => void;
  onNewThreadInProject: () => void;
  onRunProjectScript: (script: ProjectScript) => void;
  onAddProjectScript: (input: NewProjectScriptInput) => Promise<ProjectScriptActionResult>;
  onUpdateProjectScript: (
    scriptId: string,
    input: NewProjectScriptInput,
  ) => Promise<ProjectScriptActionResult>;
  onDeleteProjectScript: (scriptId: string) => Promise<ProjectScriptActionResult>;
}

export type ChatHeaderThreadAction =
  | "rename"
  | "copy"
  | "settle"
  | "unsettle"
  | "snooze"
  | "unsnooze"
  | "archive"
  | "delete";

export const chatHeaderActionTriggerLabel = (title: string) => `${title} actions`;

export function resolveChatHeaderThreadActions(input: Parameters<typeof threadActionPolicy>[0]) {
  const policy = threadActionPolicy(input);
  return [
    ...(policy.rename ? ["rename" as const] : []),
    ...(policy.copy ? ["copy" as const] : []),
    ...(policy.pinAction ? [policy.pinAction.id] : []),
    ...policy.lifecycleActions.map((action) => action.id),
    policy.destructiveAction.id,
  ];
}

export function shouldShowOpenInPicker(input: {
  readonly activeProjectName: string | undefined;
  readonly activeThreadEnvironmentId: EnvironmentId;
  readonly primaryEnvironmentId: EnvironmentId | null;
}): boolean {
  return (
    Boolean(input.activeProjectName) &&
    input.primaryEnvironmentId !== null &&
    input.activeThreadEnvironmentId === input.primaryEnvironmentId
  );
}

export const ChatHeader = memo(function ChatHeader({
  activeThreadEnvironmentId,
  activeThreadTitle,
  activeProjectName,
  activeProjectCwd,
  openInCwd,
  activeProjectScripts,
  preferredScriptId,
  keybindings,
  availableEditors,
  rightPanelOpen,
  pinningSupported,
  pinned,
  settled,
  snoozed,
  settlementSupported,
  snoozeSupported,
  onTogglePin,
  onThreadAction,
  onNewThreadInProject,
  onRunProjectScript,
  onAddProjectScript,
  onUpdateProjectScript,
  onDeleteProjectScript,
}: ChatHeaderProps) {
  const primaryEnvironmentId = usePrimaryEnvironmentId();
  const fileScripts = useT3ProjectFileScripts(
    activeThreadEnvironmentId,
    activeProjectScripts ? activeProjectCwd : null,
  );
  const showOpenInPicker = shouldShowOpenInPicker({
    activeProjectName,
    activeThreadEnvironmentId,
    primaryEnvironmentId,
  });
  const actionPolicy = threadActionPolicy({
    pinningSupported,
    pinned,
    settled,
    snoozed,
    settlementSupported,
    snoozeSupported,
  });
  return (
    <div className="@container/header-actions flex min-w-0 flex-1 items-center gap-2 sm:gap-3">
      <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden sm:gap-3">
        {/* The project always leads the header: knowing which project a
            thread lives in is priority zero, and the thread title alone
            doesn't answer it. */}
        {activeProjectName ? (
          <span className="inline-flex shrink-0 items-center gap-2">
            <Tooltip>
              <TooltipTrigger
                render={
                  <button
                    type="button"
                    aria-label={`New thread in ${activeProjectName}`}
                    onClick={onNewThreadInProject}
                    className="inline-flex min-w-0 cursor-pointer items-center gap-1.5 rounded-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                  />
                }
              >
                <ProjectFavicon
                  environmentId={activeThreadEnvironmentId}
                  cwd={activeProjectCwd ?? ""}
                  className="size-3.5"
                />
                <span className="max-w-40 truncate text-sm font-medium">{activeProjectName}</span>
              </TooltipTrigger>
              <TooltipPopup side="top">New thread in {activeProjectName}</TooltipPopup>
            </Tooltip>
            <span aria-hidden className="text-muted-foreground/40">
              /
            </span>
          </span>
        ) : null}
        <Menu>
          <Tooltip>
            <TooltipTrigger
              render={
                <MenuTrigger
                  render={
                    <button
                      type="button"
                      aria-label={chatHeaderActionTriggerLabel(activeThreadTitle)}
                      className="min-w-0 flex-1 cursor-pointer truncate rounded-sm text-left text-sm font-medium text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                    />
                  }
                />
              }
            >
              {activeThreadTitle}
            </TooltipTrigger>
            <TooltipPopup side="top">Thread actions</TooltipPopup>
          </Tooltip>
          <MenuPopup align="start">
            {actionPolicy.rename ? (
              <MenuItem onClick={() => onThreadAction("rename")}>
                <PencilIcon />
                Rename thread
              </MenuItem>
            ) : null}
            {actionPolicy.copy ? (
              <MenuItem onClick={() => onThreadAction("copy")}>
                <CopyIcon />
                Copy thread link
              </MenuItem>
            ) : null}
            {actionPolicy.pinAction ? (
              <MenuItem onClick={onTogglePin}>
                {pinned ? <PinOffIcon /> : <PinIcon />}
                {pinned ? "Unpin thread" : "Pin thread"}
              </MenuItem>
            ) : null}
            {actionPolicy.lifecycleActions.map((action) => (
              <MenuItem key={action.id} onClick={() => onThreadAction(action.id)}>
                {action.label}
              </MenuItem>
            ))}
            <MenuItem
              onClick={() => onThreadAction(actionPolicy.destructiveAction.id)}
              variant={actionPolicy.destructiveAction.id === "delete" ? "destructive" : "default"}
            >
              {actionPolicy.destructiveAction.id === "delete" ? <Trash2Icon /> : <ArchiveIcon />}
              {actionPolicy.destructiveAction.label}
            </MenuItem>
          </MenuPopup>
        </Menu>
      </div>
      <div
        data-chat-header-actions
        className={cn(
          "flex shrink-0 items-center justify-end gap-2 @3xl/header-actions:gap-3",
          rightPanelOpen ? "pr-0" : "pr-16",
        )}
      >
        {activeProjectScripts && (
          <ProjectScriptsControl
            scripts={activeProjectScripts}
            fileScripts={fileScripts}
            keybindings={keybindings}
            preferredScriptId={preferredScriptId}
            onRunScript={onRunProjectScript}
            onAddScript={onAddProjectScript}
            onUpdateScript={onUpdateProjectScript}
            onDeleteScript={onDeleteProjectScript}
          />
        )}
        {showOpenInPicker && (
          <OpenInPicker
            environmentId={activeThreadEnvironmentId}
            keybindings={keybindings}
            availableEditors={availableEditors}
            openInCwd={openInCwd}
          />
        )}
      </div>
    </div>
  );
});
