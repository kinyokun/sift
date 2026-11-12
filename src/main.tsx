import { createRoot } from "react-dom/client";
import "./styles.css";

async function boot() {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("bridge")) await import("./dev/mock");
  const { App } = await import("./App");
  createRoot(document.getElementById("root")!).render(<App />);
}

boot();
