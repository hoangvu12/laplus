import { RouterProvider } from "@tanstack/react-router";

import { ElectronBrowserHost } from "./browser/ElectronBrowserHost";
import { DesktopWindowControls } from "./components/DesktopWindowControls";
import { PreviewAutomationHosts } from "./components/preview/PreviewAutomationHosts";
import { AppAtomRegistryProvider } from "./rpc/atomRegistry";
import type { AppRouter } from "./router";

/**
 * Owns renderer-wide providers. The Electron browser host intentionally sits
 * outside the router so its webviews survive route transitions, but it must
 * share the same atom registry as routed UI.
 *
 * The window controls sit outside the router for the same reason: they belong
 * to the window rather than to a route, and every route has the same top-right
 * corner. They render nothing outside laplus's own desktop shell.
 */
export function AppRoot({ router }: { readonly router: AppRouter }) {
  return (
    <AppAtomRegistryProvider>
      <RouterProvider router={router} />
      <PreviewAutomationHosts />
      <ElectronBrowserHost />
      <DesktopWindowControls />
    </AppAtomRegistryProvider>
  );
}
