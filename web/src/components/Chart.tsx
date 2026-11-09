import { AgCharts } from "ag-charts-react";
import { AllCommunityModule, ModuleRegistry, type AgChartOptions, type AgChartTheme } from "ag-charts-community";

// AG Charts Community only.
ModuleRegistry.registerModules([AllCommunityModule]);

/** Chart roles (text wears ink tokens; marks carry identity). */
export const viz = {
  ink: "#171717",
  text: "#0A0A0A",
  textMuted: "#737373",
  grid: "#EDEDED",
  surface: "#FFFFFF",
  green: "#2AD829",
  greenFill: "rgba(42, 216, 41, 0.22)",
};

export const chartTheme: AgChartTheme = {
  baseTheme: "ag-default",
  params: {
    fontFamily: "Inter, system-ui, sans-serif",
    fontSize: 12,
    textColor: viz.text,
    subtleTextColor: viz.textMuted,
    backgroundColor: viz.surface,
    axisLineColor: "#D4D4D4",
    axisLabelColor: viz.textMuted,
    gridLineColor: viz.grid,
    foregroundColor: viz.text,
  },
};

export function Chart({ options, height = 280, label }: { options: AgChartOptions; height?: number; label: string }) {
  return (
    <div className="chart" style={{ height }} role="img" aria-label={label}>
      <AgCharts options={{ theme: chartTheme, background: { fill: viz.surface }, ...options }} style={{ height: "100%" }} />
    </div>
  );
}
