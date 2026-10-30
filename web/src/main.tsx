import { render } from "preact";
import { App } from "./app";
import { MeProvider } from "./lib/me";
import { registerServiceWorker } from "./lib/pwa";
import { startRouter } from "./lib/router";
import "./styles.css";

startRouter();
registerServiceWorker();

const root = document.getElementById("app");
if (root) {
  render(
    <MeProvider>
      <App />
    </MeProvider>,
    root,
  );
}
