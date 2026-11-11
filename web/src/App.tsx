import { lazy, Suspense } from "react";
import { BrowserRouter, Navigate, Outlet, Route, Routes, useParams } from "react-router-dom";
import { DEFAULT_SLUG, Footer, TopBar } from "./components/TopBar";
import { Spinner } from "./components/ui";
import { PromoterPage } from "./features/promoter/PromoterPage";
import { ScoutPage } from "./features/scout/ScoutPage";
import { ChatStoreProvider } from "./lib/chatStore";
import { DemoProvider } from "./lib/demo";
import { VenueProvider } from "./lib/venue";

// The console pulls in AG Grid and AG Charts: load it only when the owner view opens.
const ConsolePage = lazy(() => import("./features/console/ConsolePage").then((m) => ({ default: m.ConsolePage })));

/** Shared by the promoter view and the owner console, so switching roles keeps the venue loaded. */
function VenueLayout() {
  const { slug = DEFAULT_SLUG } = useParams();
  return (
    <VenueProvider key={slug} slug={slug}>
      <div className="app">
        <TopBar />
        <main className="main">
          <Suspense
            fallback={
              <div className="page-loading">
                <Spinner label="Loading…" />
              </div>
            }
          >
            <Outlet />
          </Suspense>
        </main>
        <Footer />
      </div>
    </VenueProvider>
  );
}

/** Pages that are not about one venue (the AI venue scout). */
function PlainLayout() {
  return (
    <div className="app">
      <TopBar />
      <main className="main">
        <Outlet />
      </main>
      <Footer />
    </div>
  );
}

function NotFound() {
  return (
    <div className="app">
      <TopBar />
      <main className="main">
        <div className="empty">
          <p className="empty__title">Page not found</p>
          <p className="empty__body">
            <a href="/find">Open the AI venue scout</a> or <a href={`/v/${DEFAULT_SLUG}`}>the Blue Lantern Hall card</a>
          </p>
        </div>
      </main>
      <Footer />
    </div>
  );
}

export function App() {
  return (
    <BrowserRouter>
      <DemoProvider>
        <ChatStoreProvider>
          <Routes>
            <Route path="/" element={<Navigate to="/find" replace />} />
            <Route element={<PlainLayout />}>
              <Route path="/find" element={<ScoutPage />} />
            </Route>
            <Route element={<VenueLayout />}>
              <Route path="/v/:slug" element={<PromoterPage />} />
              <Route path="/console/:slug" element={<ConsolePage />} />
            </Route>
            <Route path="*" element={<NotFound />} />
          </Routes>
        </ChatStoreProvider>
      </DemoProvider>
    </BrowserRouter>
  );
}
