import { SquarePenIcon, XIcon } from "lucide-react";
import { memo, useCallback, useMemo, useState, type KeyboardEvent, type MouseEvent } from "react";
import { cn } from "~/lib/utils";
import {
  composerDraftHasUserContent,
  DraftId,
  useComposerDraftStore,
  type ComposerThreadDraftState,
  type DraftSessionState,
} from "../../composerDraftStore";
import { ProjectFavicon } from "../ProjectFavicon";

interface DraftRowData {
  draftId: DraftId;
  session: DraftSessionState;
  composer: ComposerThreadDraftState;
}

const SavedDraftRow = memo(function SavedDraftRow(
  props: DraftRowData & {
    projectTitle: string;
    projectCwd: string;
    active: boolean;
    onNavigate: (draftId: DraftId) => void;
    onDiscard: (draftId: DraftId) => void;
  },
) {
  const firstLine = props.composer.prompt.trim().split("\n", 1)[0] ?? "";
  const attachmentCount =
    Math.max(props.composer.images.length, props.composer.persistedAttachments.length) +
    props.composer.terminalContexts.length +
    props.composer.elementContexts.length +
    props.composer.previewAnnotations.length +
    props.composer.reviewComments.length;
  const preview = firstLine || `${attachmentCount} attachment${attachmentCount === 1 ? "" : "s"}`;
  const activate = () => props.onNavigate(props.draftId);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if ((event.target as HTMLElement).closest("button")) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activate();
    }
  };
  const discard = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    props.onDiscard(props.draftId);
  };
  return (
    <li className="list-none py-0.5">
      <div
        role="button"
        tabIndex={0}
        data-testid="sidebar-draft-row"
        onClick={activate}
        onKeyDown={onKeyDown}
        className={cn(
          "group/saved-draft cursor-pointer rounded-md px-2 py-1.5 outline-none hover:bg-sidebar-row-hover focus-visible:ring-2 focus-visible:ring-ring",
          props.active && "bg-sidebar-row-active",
        )}
      >
        <div className="flex min-w-0 items-center gap-1.5">
          <SquarePenIcon className="size-3 shrink-0 text-amber-600 dark:text-amber-300/80" />
          <ProjectFavicon
            environmentId={props.session.environmentId}
            cwd={props.projectCwd}
            className="size-4 shrink-0"
          />
          <span className="min-w-0 flex-1 truncate text-xs font-medium text-sidebar-muted-foreground">
            {props.projectTitle}
          </span>
          <button
            type="button"
            aria-label="Discard draft"
            title="Discard draft"
            onClick={discard}
            className="pointer-events-none rounded px-1 text-sidebar-muted-foreground opacity-0 hover:text-sidebar-foreground focus-visible:pointer-events-auto focus-visible:opacity-100 group-hover/saved-draft:pointer-events-auto group-hover/saved-draft:opacity-100"
          >
            <XIcon className="size-3" />
          </button>
        </div>
        <div className="mt-0.5 truncate text-sm font-medium text-sidebar-foreground">{preview}</div>
      </div>
    </li>
  );
});

export const SavedDraftShelf = memo(function SavedDraftShelf(props: {
  activeDraftId: string | null;
  projectDisplayNameByKey: ReadonlyMap<string, string>;
  projectCwdByKey: ReadonlyMap<string, string>;
  scopedProjectKeys?: ReadonlySet<string> | null;
  onNavigate: (draftId: DraftId) => void;
}) {
  const sessions = useComposerDraftStore((state) => state.draftThreadsByThreadKey);
  const composers = useComposerDraftStore((state) => state.draftsByThreadKey);
  const clearDraftThread = useComposerDraftStore((state) => state.clearDraftThread);
  const [frozen, setFrozen] = useState<{ id: string | null; row: DraftRowData | null }>({
    id: null,
    row: null,
  });
  if (frozen.id !== props.activeDraftId) {
    const id = props.activeDraftId;
    const session = id ? sessions[id] : undefined;
    const composer = id ? composers[id] : undefined;
    setFrozen({
      id,
      row:
        id && session && composer && composerDraftHasUserContent(composer)
          ? { draftId: DraftId.make(id), session, composer }
          : null,
    });
  }
  const rows = useMemo(
    () =>
      Object.entries(sessions)
        .flatMap(([id, session]) => {
          if (session.promotedTo != null) return [];
          const projectKey = `${session.environmentId}:${session.projectId}`;
          if (props.scopedProjectKeys && !props.scopedProjectKeys.has(projectKey)) return [];
          if (id === props.activeDraftId) return frozen.id === id && frozen.row ? [frozen.row] : [];
          const composer = composers[id];
          return composerDraftHasUserContent(composer)
            ? [{ draftId: DraftId.make(id), session, composer: composer! }]
            : [];
        })
        .sort((a, b) => b.session.createdAt.localeCompare(a.session.createdAt)),
    [composers, frozen, props.activeDraftId, props.scopedProjectKeys, sessions],
  );
  const discard = useCallback((id: DraftId) => clearDraftThread(id), [clearDraftThread]);
  if (rows.length === 0) return null;
  return (
    <>
      {rows.map((row) => {
        const key = `${row.session.environmentId}:${row.session.projectId}`;
        return (
          <SavedDraftRow
            key={row.draftId}
            {...row}
            active={row.draftId === props.activeDraftId}
            projectTitle={props.projectDisplayNameByKey.get(key) ?? "Unknown project"}
            projectCwd={props.projectCwdByKey.get(key) ?? ""}
            onNavigate={props.onNavigate}
            onDiscard={discard}
          />
        );
      })}
      <li
        aria-hidden
        data-testid="sidebar-draft-divider"
        className="mx-2.5 my-1.5 h-px list-none bg-sidebar-border/60"
      />
    </>
  );
});
