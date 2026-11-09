import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Dev server: the Rust API runs separately. Override the target with DC_BACKEND=http://127.0.0.1:8787.
// (This file is not type-checked by `tsc`; Vite transpiles it.)
const backend = process.env.DC_BACKEND || "http://127.0.0.1:8788";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    // Set DC_POLL=1 where file-system events don't reach Vite (sandboxes, network drives).
    watch: process.env.DC_POLL === "1" ? { usePolling: true, interval: 300 } : undefined,
    proxy: {
      "/api": { target: backend },
      "/healthz": { target: backend },
    },
  },
  preview: {
    proxy: {
      "/api": { target: backend },
      "/healthz": { target: backend },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The Bryntum trial bundle is shipped unminified by the vendor, so its chunk is large.
    chunkSizeWarningLimit: 7000,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes("node_modules/@bryntum/")) return "bryntum";
          if (id.includes("node_modules/ag-charts")) return "ag-charts";
          if (id.includes("node_modules/ag-grid") || id.includes("node_modules/ag-stack")) return "ag-grid";
          if (id.includes("node_modules/react") || id.includes("node_modules/scheduler/")) return "react";
          return undefined;
        },
      },
    },
  },
});
