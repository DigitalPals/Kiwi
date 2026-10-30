/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" when the dev server serves mock fixtures. */
  readonly VITE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

interface Navigator {
  /** iOS Safari: true when launched from the Home Screen, false in a browser tab. */
  readonly standalone?: boolean;
}

declare const __APP_VERSION__: string;
