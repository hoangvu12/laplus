// PROTOTYPE — throw away after choosing a subagent inspection layout.
// Three variants, switchable via ?variant=, on /subagent-prototype.
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  BotIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CircleIcon,
  FileCodeIcon,
  FileDiffIcon,
  SearchIcon,
  TerminalIcon,
  WrenchIcon,
  XIcon,
} from "lucide-react";
import { type ReactNode, useEffect } from "react";

type Variant = "A" | "B" | "C";

export const Route = createFileRoute("/subagent-prototype")({
  validateSearch: (search: Record<string, unknown>): { variant: Variant } => ({
    variant: search.variant === "B" || search.variant === "C" ? search.variant : "A",
  }),
  component: SubagentPrototype,
});

const agents = [
  { name: "reviewer", task: "Review auth boundaries", state: "running", turn: "Current turn" },
  { name: "test-scout", task: "Find missing coverage", state: "running", turn: "Current turn" },
  { name: "researcher", task: "Compare upstream behavior", state: "done", turn: "Current turn" },
  { name: "explorer", task: "Map provider event shapes", state: "done", turn: "Earlier turn" },
] as const;

function SubagentPrototype() {
  const { variant } = Route.useSearch();
  return (
    <div className="min-h-screen bg-[#101113] text-[#e8e8e8]">
      {variant === "A" ? <FocusedInspector /> : null}
      {variant === "B" ? <PanelNavigator /> : null}
      {variant === "C" ? <FullFocusSession /> : null}
      {import.meta.env.DEV ? <PrototypeSwitcher variant={variant} /> : null}
    </div>
  );
}

function AppSidebar() {
  return (
    <aside className="hidden w-52 shrink-0 border-r border-white/8 bg-[#161719] p-3 md:block">
      <div className="mb-5 flex items-center gap-2 px-2 text-sm font-semibold">
        <span className="grid size-6 place-items-center rounded-md bg-white text-xs text-black">
          L
        </span>
        laplus
      </div>
      <div className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-white/35">
        Threads
      </div>
      <div className="rounded-md bg-white/8 px-2.5 py-2 text-xs">Subagent visibility</div>
      <div className="px-2.5 py-2 text-xs text-white/45">Image attachments</div>
      <div className="px-2.5 py-2 text-xs text-white/45">Provider parity</div>
    </aside>
  );
}

function Conversation({ selected = "reviewer" }: { selected?: string }) {
  return (
    <main className="min-w-0 flex-1 overflow-hidden bg-[#101113]">
      <header className="flex h-11 items-center justify-between border-b border-white/8 px-5">
        <span className="truncate text-sm font-medium">Subagent visibility</span>
        <span className="rounded bg-white/5 px-2 py-1 text-[10px] text-white/45">main · opus</span>
      </header>
      <div className="mx-auto max-w-2xl space-y-6 px-5 py-7 text-[13px] leading-6">
        <div className="ml-auto max-w-lg rounded-2xl rounded-br-sm bg-[#26282c] px-4 py-2.5">
          Research how other tools expose subagent work, then recommend a layout.
        </div>
        <p className="text-white/78">
          I’ll compare the strongest interfaces and inspect our current rendering.
        </p>
        <section className="rounded-lg border border-white/10 bg-[#17181b] p-2">
          <div className="flex items-center justify-between px-2 py-1.5">
            <span className="text-xs font-semibold">Subagents</span>
            <span className="text-[10px] text-white/42">2 running · 1 done</span>
          </div>
          {agents.slice(0, 3).map((agent) => (
            <div
              key={agent.name}
              className={`flex items-center gap-2 rounded-md px-2 py-2 ${agent.name === selected ? "bg-[#2a2c31]" : "hover:bg-white/5"}`}
            >
              <Status state={agent.state} />
              <div className="min-w-0 flex-1">
                <div className="flex gap-2 text-xs">
                  <span className="font-medium">{agent.name}</span>
                  <span className="truncate text-white/42">{agent.task}</span>
                </div>
                <div className="truncate text-[11px] text-white/35">
                  {agent.state === "done"
                    ? "Result: 3 useful UI patterns found"
                    : "Reading provider runtime code…"}
                </div>
              </div>
              <ChevronRightIcon className="size-3.5 text-white/30" />
            </div>
          ))}
        </section>
        <p className="text-white/78">
          The child sessions are still working. You can inspect either one without leaving this
          turn.
        </p>
      </div>
    </main>
  );
}

function WorkStream() {
  return (
    <div className="space-y-3 p-4 text-xs leading-5">
      <p className="text-white/75">
        I’ll trace authentication boundaries and check where refresh tokens are accepted.
      </p>
      <Work
        icon={<SearchIcon />}
        title="Searched code"
        detail={'rg "refresh_token|authorize" server/crates'}
      />
      <Work
        icon={<FileCodeIcon />}
        title="Read auth.rs"
        detail="server/crates/laplus-server/src/auth.rs"
      />
      <p className="text-white/75">
        The browser session and API token paths enforce different scopes. I’m checking their tests.
      </p>
      <Work
        icon={<TerminalIcon />}
        title="Ran focused tests"
        detail="cargo test -p laplus-server auth::tests"
        live
      />
      <Work
        icon={<WrenchIcon />}
        title="Inspecting token claims"
        detail="Comparing audience and workspace scope"
        live
      />
    </div>
  );
}

function Work(props: { icon: ReactNode; title: string; detail: string; live?: boolean }) {
  return (
    <div className="flex gap-2 rounded-md border border-white/8 bg-white/[0.025] px-2.5 py-2">
      <span className="mt-0.5 text-white/40 [&>svg]:size-3.5">{props.icon}</span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between">
          <span className="font-medium text-white/72">{props.title}</span>
          {props.live ? (
            <span className="text-[9px] text-amber-300/80">RUNNING</span>
          ) : (
            <CheckIcon className="size-3 text-emerald-400" />
          )}
        </div>
        <div className="truncate font-mono text-[10px] text-white/38">{props.detail}</div>
      </div>
    </div>
  );
}

function InspectorHeader({ navigator = false }: { navigator?: boolean }) {
  return (
    <>
      <div className="flex h-10 items-center gap-2 border-b border-white/8 px-3">
        {navigator ? (
          <>
            <BotIcon className="size-3.5 text-violet-300" />
            <span className="text-xs font-semibold">Subagents</span>
            <span className="ml-auto flex items-center gap-1 text-[10px] text-white/35">
              4 in thread <ChevronDownIcon className="size-3" />
            </span>
            <XIcon className="ml-1 size-3.5 text-white/35" />
          </>
        ) : (
          <>
            <div className="flex h-8 items-center gap-1.5 rounded-md bg-white/9 px-2 text-[11px] font-medium">
              <BotIcon className="size-3.5 text-violet-300" /> reviewer
              <XIcon className="ml-1 size-3 text-white/30" />
            </div>
            <div className="flex h-8 items-center gap-1.5 px-2 text-[11px] text-white/42">
              <FileCodeIcon className="size-3.5" /> auth.rs{" "}
              <XIcon className="size-3 text-white/25" />
            </div>
            <div className="flex h-8 items-center gap-1.5 px-2 text-[11px] text-white/42">
              <FileDiffIcon className="size-3.5" /> Diff <XIcon className="size-3 text-white/25" />
            </div>
          </>
        )}
      </div>
    </>
  );
}

function FocusedInspector() {
  return (
    <div className="flex h-screen">
      <AppSidebar />
      <Conversation />
      <aside className="flex w-[42%] min-w-[340px] max-w-[560px] flex-col border-l border-white/10 bg-[#151619]">
        <InspectorHeader />
        <div className="min-h-0 flex-1 overflow-y-auto">
          <WorkStream />
        </div>
        <div className="flex h-10 items-center justify-between border-t border-white/8 px-3 text-[10px] text-white/38">
          <span>Live · following</span>
          <span>‹ Previous&nbsp;&nbsp; Next ›</span>
        </div>
      </aside>
    </div>
  );
}

function PanelNavigator() {
  return (
    <div className="flex h-screen">
      <AppSidebar />
      <Conversation />
      <aside className="flex w-[48%] min-w-[440px] max-w-[680px] flex-col border-l border-white/10 bg-[#151619]">
        <InspectorHeader navigator />
        <div className="flex min-h-0 flex-1">
          <nav className="w-40 shrink-0 overflow-y-auto border-r border-white/8 p-2">
            <p className="px-2 py-1 text-[9px] font-semibold uppercase tracking-wider text-white/28">
              Current turn
            </p>
            {agents.slice(0, 3).map((agent) => (
              <AgentNav key={agent.name} agent={agent} />
            ))}
            <p className="mt-3 px-2 py-1 text-[9px] font-semibold uppercase tracking-wider text-white/28">
              Earlier turn
            </p>
            <AgentNav agent={agents[3]} />
          </nav>
          <div className="min-w-0 flex-1 overflow-y-auto">
            <div className="border-b border-white/8 px-4 py-3">
              <div className="text-sm font-semibold">reviewer</div>
              <div className="text-[11px] text-white/42">Review auth boundaries</div>
            </div>
            <WorkStream />
          </div>
        </div>
      </aside>
    </div>
  );
}

function AgentNav({ agent }: { agent: (typeof agents)[number] }) {
  return (
    <div className={`mb-1 rounded-md px-2 py-2 ${agent.name === "reviewer" ? "bg-white/9" : ""}`}>
      <div className="flex items-center gap-1.5 text-[11px]">
        <Status state={agent.state} />
        <span className="truncate">{agent.name}</span>
      </div>
      <div className="mt-0.5 truncate pl-3.5 text-[9px] text-white/30">{agent.task}</div>
    </div>
  );
}

function FullFocusSession() {
  return (
    <div className="flex h-screen">
      <AppSidebar />
      <section className="flex min-w-0 flex-1 flex-col bg-[#101113]">
        <header className="flex h-12 items-center gap-3 border-b border-white/8 px-4">
          <ChevronLeftIcon className="size-4 text-white/45" />
          <span className="text-xs text-white/38">Subagent visibility</span>
          <span className="text-white/18">/</span>
          <Status state="running" />
          <span className="text-sm font-semibold">reviewer</span>
          <span className="text-xs text-white/35">Review auth boundaries</span>
          <div className="ml-auto flex gap-2">
            <button className="rounded border border-white/10 px-2 py-1 text-[10px] text-white/48">
              View diff
            </button>
            <button className="rounded border border-white/10 px-2 py-1 text-[10px] text-white/48">
              Next agent
            </button>
          </div>
        </header>
        <div className="mx-auto min-h-0 w-full max-w-3xl flex-1 overflow-y-auto py-5">
          <WorkStream />
        </div>
        <div className="border-t border-white/8 px-4 py-2 text-center text-[10px] text-white/30">
          Read-only child session · Live
        </div>
      </section>
    </div>
  );
}

function Status({ state }: { state: string }) {
  return state === "done" ? (
    <span className="grid size-3 place-items-center rounded-full bg-emerald-400/20">
      <CheckIcon className="size-2.5 text-emerald-400" />
    </span>
  ) : (
    <CircleIcon className="size-3 fill-amber-300/20 text-amber-300" />
  );
}

const variantNames: Record<Variant, string> = {
  A: "Focused inspector",
  B: "Panel navigator",
  C: "Full-focus session",
};

function PrototypeSwitcher({ variant }: { variant: Variant }) {
  const navigate = useNavigate({ from: "/subagent-prototype" });
  const variants: Variant[] = ["A", "B", "C"];
  const cycle = (offset: number) => {
    const index = variants.indexOf(variant);
    const next = variants[(index + offset + variants.length) % variants.length] ?? "A";
    void navigate({ search: { variant: next }, replace: true });
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.matches("input, textarea, [contenteditable]")) return;
      if (event.key === "ArrowLeft") cycle(-1);
      if (event.key === "ArrowRight") cycle(1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });
  return (
    <div className="fixed bottom-5 left-1/2 z-50 flex -translate-x-1/2 items-center gap-3 rounded-full border border-white/15 bg-black/90 p-1.5 shadow-2xl">
      <button
        onClick={() => cycle(-1)}
        className="grid size-7 place-items-center rounded-full hover:bg-white/10"
        aria-label="Previous variant"
      >
        <ChevronLeftIcon className="size-4" />
      </button>
      <span className="min-w-36 text-center text-xs">
        <b>{variant}</b> — {variantNames[variant]}
      </span>
      <button
        onClick={() => cycle(1)}
        className="grid size-7 place-items-center rounded-full hover:bg-white/10"
        aria-label="Next variant"
      >
        <ChevronRightIcon className="size-4" />
      </button>
    </div>
  );
}
