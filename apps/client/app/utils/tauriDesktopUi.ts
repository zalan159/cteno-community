import { Platform } from 'react-native';

/**
 * Metro (web) dev server serves its own HTML template, so desktop-only CSS must
 * be injected at runtime to affect desktop web shells (Tauri today, potentially
 * others later). This is intentionally NOT tied to any Tauri globals so the
 * same codebase can ship to iOS/Android native unchanged.
 */
function isDesktopWeb(): boolean {
  if (Platform.OS !== 'web') return false;
  if (typeof window === 'undefined') return false;

  // "Desktop-like" input heuristics. This avoids relying on Tauri globals.
  if (typeof window.matchMedia === 'function') {
    if (window.matchMedia('(hover: hover) and (pointer: fine), (min-width: 900px)').matches) return true;
  }

  // Fallback: wide viewport.
  return window.innerWidth >= 900;
}

function ensureDesktopCss(scale: number) {
  const id = 'cteno-desktop-web-ui';
  const existing = document.getElementById(id) as HTMLStyleElement | null;
  const style = existing ?? document.createElement('style');
  style.id = id;
  style.textContent = `
@media (hover: hover) and (pointer: fine), (min-width: 900px) {
  body {
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  text-rendering: optimizeLegibility;
  }
  #root {
  transform: scale(${scale});
  transform-origin: 0 0;
  width: calc(100% / ${scale});
  height: calc(100% / ${scale});
  }
}
  `.trim();

  if (!existing) {
    document.head.appendChild(style);
  }
}

// Apply ASAP (web only). Native iOS/Android builds won't hit this code path.
if (Platform.OS === 'web' && typeof window !== 'undefined' && typeof document !== 'undefined') {
  const SCALE = 0.87;
  if (isDesktopWeb()) ensureDesktopCss(SCALE);
}
