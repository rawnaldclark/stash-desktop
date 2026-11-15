/**
 * Appearance: Dark, Light or the system's, plus pure black (dark only). Kept in this window (localStorage), so it applies before
 * anything else loads: app.html's head script reads the same two keys and sets the same attributes before the first paint.
 * CSS: every `@media (prefers-color-scheme: light)` block is scoped by vite.config.ts's `themeSwitch` so `html[data-theme]` wins
 * over the system; `html[data-black]` turns the dark surfaces black whenever dark is showing (tokens.css). Only Settings loads this
 * module; the shell reads the choice back from `html[data-theme]`.
 */
export type ThemeChoice = 'system' | 'dark' | 'light';
export type Appearance = { theme: ThemeChoice; black: boolean };

const THEME_KEY = 'stash.theme';
const BLACK_KEY = 'stash.black';

export function readAppearance(): Appearance {
	try {
		const t = localStorage.getItem(THEME_KEY);
		return { theme: t === 'dark' || t === 'light' ? t : 'system', black: localStorage.getItem(BLACK_KEY) === '1' };
	} catch {
		return { theme: 'system', black: false };
	}
}

export function saveAppearance(a: Appearance): void {
	try {
		if (a.theme === 'system') localStorage.removeItem(THEME_KEY);
		else localStorage.setItem(THEME_KEY, a.theme);
		if (a.black) localStorage.setItem(BLACK_KEY, '1');
		else localStorage.removeItem(BLACK_KEY);
	} catch {
		/* storage blocked: this visit only */
	}
}

/** Puts the choice on <html>. */
export function applyAppearance(a: Appearance): void {
	const d = document.documentElement;
	if (a.theme === 'system') delete d.dataset.theme;
	else d.dataset.theme = a.theme;
	if (a.black) d.dataset.black = '1';
	else delete d.dataset.black;
}
