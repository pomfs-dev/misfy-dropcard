import { AgGridReact } from "ag-grid-react";
import { AllCommunityModule, ModuleRegistry, themeQuartz, type ColDef, type GridOptions } from "ag-grid-community";

// AG Grid Community only (no enterprise modules, no row grouping, no sparklines).
ModuleRegistry.registerModules([AllCommunityModule]);

export const gridTheme = themeQuartz.withParams({
  fontFamily: "Inter, system-ui, sans-serif",
  fontSize: 13,
  accentColor: "#000000",
  foregroundColor: "#0A0A0A",
  backgroundColor: "#FFFFFF",
  borderColor: "#E5E5E5",
  headerBackgroundColor: "#FAFAFA",
  headerTextColor: "#525252",
  headerFontWeight: 600,
  rowHoverColor: "#F5F5F5",
  selectedRowBackgroundColor: "#F0FDF0",
  oddRowBackgroundColor: "#FFFFFF",
  pinnedRowBackgroundColor: "#F5F5F5",
  pinnedRowFontWeight: 700,
  wrapperBorderRadius: 12,
  borderRadius: 8,
  spacing: 7,
  headerColumnBorder: false,
  columnBorder: false,
});

type Props<T> = {
  rows: T[];
  columns: ColDef<T>[];
  pinnedBottom?: Partial<Record<keyof T | string, unknown>>[];
  height?: number | "auto";
  getRowId?: (row: T) => string;
  emptyText?: string;
  options?: GridOptions<T>;
};

export function Grid<T>({ rows, columns, pinnedBottom, height = 420, getRowId, emptyText, options }: Props<T>) {
  const auto = height === "auto";
  return (
    <div className="grid-wrap" style={auto ? undefined : { height }}>
      <AgGridReact<T>
        theme={gridTheme}
        rowData={rows}
        columnDefs={columns}
        pinnedBottomRowData={pinnedBottom as T[] | undefined}
        domLayout={auto ? "autoHeight" : "normal"}
        getRowId={getRowId ? (p) => (p.rowPinned ? `pinned-${p.rowPinned}` : getRowId(p.data)) : undefined}
        defaultColDef={{ sortable: true, resizable: true, filter: true, minWidth: 90, suppressHeaderMenuButton: false }}
        animateRows
        suppressCellFocus
        overlayNoRowsTemplate={`<span class="grid-empty">${emptyText ?? "No rows yet"}</span>`}
        {...options}
      />
    </div>
  );
}
