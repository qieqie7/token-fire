import React from "react";
import { createRoot } from "react-dom/client";
import "./design-system/tokens.css";
import "./style.css";
import { App } from "./app/App";

const root = document.querySelector<HTMLDivElement>("#app");

if (!root) {
  throw new Error("TokenFire root element #app was not found");
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
