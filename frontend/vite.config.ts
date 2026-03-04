import { readFileSync } from "node:fs";
import { defineConfig, type Plugin } from "vite";
import { fresh } from "@fresh/plugin-vite";
import tailwindcss from "@tailwindcss/vite";

// Fixes broken sourcemaps in @opentelemetry/api by stripping the
// sourceMappingURL comment during file loading. The package ships .map
// files that reference TypeScript sources not included in the npm
// distribution. When Rollup reads the file it extracts the sourcemap
// and later fails to resolve original locations through it, producing
// "Can't resolve original location of error" warnings.
//
// Using `load` (not `transform`) because Rollup extracts sourcemaps from
// the file content during the load phase, before transform hooks run.
function fixOpenTelemetrySourcemaps(): Plugin {
  return {
    name: "fix-opentelemetry-sourcemaps",
    enforce: "pre",
    load(id) {
      if (
        id.includes("@opentelemetry") && id.endsWith(".js") &&
        !id.includes("\0")
      ) {
        const code = readFileSync(id, "utf-8");
        if (code.includes("//# sourceMappingURL=")) {
          return {
            code: code.replace(/\/\/# sourceMappingURL=.*$/m, ""),
            map: null,
          };
        }
      }
    },
  };
}

export default defineConfig({
  plugins: [fixOpenTelemetrySourcemaps(), fresh(), tailwindcss()],
});
