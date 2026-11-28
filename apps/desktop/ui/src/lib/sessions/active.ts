/**
 * Whether this window hosts a session now (session.svelte.ts sets it). The window's timers run at full pace while it is hidden
 * in the tray (WebView2's throttling is off so a session goes on: the shell's main.rs); without a session the player is asked
 * now and then while hidden (app.ts).
 */
export const sessionActive = { hosting: false };
