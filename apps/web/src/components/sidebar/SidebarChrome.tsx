import { ChartNoAxesColumnIcon, SettingsIcon } from "lucide-react";
import { memo, useCallback } from "react";
import { useNavigate } from "@tanstack/react-router";

import laplusBannerUrl from "../../assets/laplus-banner.jpg";
import { cn } from "../../lib/utils";
import {
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarTrigger,
  useSidebar,
} from "../ui/sidebar";
import { SidebarProviderUpdatePill } from "./SidebarProviderUpdatePill";
import { SidebarUpdatePill } from "./SidebarUpdatePill";

export const SidebarChromeHeader = memo(function SidebarChromeHeader() {
  return (
    <SidebarHeader
      className={cn(
        "@container/sidebar-header relative h-[var(--workspace-topbar-height)] shrink-0 flex-row items-center px-3 py-0 md:px-0",
      )}
      // Unconditional because the attribute is inert without Tauri's injected
      // listener — there is nothing to gate it on in a browser. Ticket 27.
      data-tauri-drag-region="deep"
    >
      <SidebarBanner />
      <SidebarTrigger
        className={cn(
          "relative z-10 md:hidden",
          "[:hover,[data-pressed]]:bg-white/15 focus-visible:ring-white/90 focus-visible:ring-offset-blue-700 [&_svg]:stroke-white/90! [&_svg]:opacity-100! [&_svg]:hover:stroke-white!",
        )}
      />
    </SidebarHeader>
  );
});

/**
 * The header art. It borrows `.sidebar-stage-backdrop` from the stage-channel
 * backdrops so the mask and the ramp to the sidebar's own surface colour are
 * the ones already tuned there — the only difference is that this one is a
 * photograph, and that it is on for every channel rather than for nightly and
 * dev. The art is right-anchored because the subject is on that side of the
 * frame; the flat left half is the header's breathing room. It is the whole of
 * the branding — there is no wordmark beside it.
 */
function SidebarBanner() {
  return (
    <div
      aria-hidden
      className="sidebar-stage-backdrop pointer-events-none absolute inset-x-0 top-0 z-0 h-20 select-none overflow-hidden"
    >
      <img alt="" className="h-full w-full object-cover object-right" src={laplusBannerUrl} />
    </div>
  );
}

export const SidebarChromeFooter = memo(function SidebarChromeFooter() {
  const navigate = useNavigate();
  const { isMobile, setOpenMobile } = useSidebar();
  const handleSettingsClick = useCallback(() => {
    if (isMobile) {
      setOpenMobile(false);
    }
    void navigate({ to: "/settings" });
  }, [isMobile, navigate, setOpenMobile]);
  const handleUsageClick = useCallback(() => {
    if (isMobile) setOpenMobile(false);
    void navigate({ to: "/usage" });
  }, [isMobile, navigate, setOpenMobile]);

  return (
    <SidebarFooter className="p-2">
      <SidebarProviderUpdatePill />
      <SidebarUpdatePill />
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton
            size="sm"
            className="h-8 items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-sidebar-muted-foreground/80 hover:bg-sidebar-row-hover hover:text-sidebar-foreground"
            onClick={handleUsageClick}
          >
            <ChartNoAxesColumnIcon className="size-4.5 shrink-0" />
            <span>Usage</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
        <SidebarMenuItem>
          <SidebarMenuButton
            size="sm"
            className="h-8 items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-sidebar-muted-foreground/80 hover:bg-sidebar-row-hover hover:text-sidebar-foreground"
            onClick={handleSettingsClick}
          >
            <SettingsIcon className="size-4.5 shrink-0" />
            <span>Settings</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarFooter>
  );
});
