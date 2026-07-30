import "vite-plus/test/config";
import { defineConfig, mergeConfig } from "vite-plus";

import baseConfig from "../../vite.config.ts";

// One entry, no dependencies to decide about: `src/bin.ts` imports two local
// modules and the manifest, and everything else it uses is Node's. `src/release.ts`
// is deliberately absent — it is run by the release workflow with `node`,
// straight from the checkout, and is not part of what gets published.
export default mergeConfig(
  baseConfig,
  defineConfig({
    pack: {
      entry: ["src/bin.ts"],
      outDir: "dist",
      clean: true,
    },
  }),
);
