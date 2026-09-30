import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

async function boot() {
  // Dev-only: `/?mock=5000` runs the UI against an in-browser fake catalog (used by Playwright tests).
  const mock = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("mock") : null;
  if (mock !== null) {
    const { installMockBackend } = await import("./testing/mockBackend");
    installMockBackend(Number(mock) || 5000);
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
void boot();
