import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import type { ChildNode, Plugin as PostcssPlugin, Rule } from 'postcss';
import { defineConfig } from 'vitest/config';
import pkg from './package.json' with { type: 'json' };

/**
 * Appearance (src/lib/ui/theme.ts): the stylesheets keep writing their light colours as `@media (prefers-color-scheme: light) { … }`;
 * this turns every such block into two, so a choice made in Settings beats the system's:
 *   @media (prefers-color-scheme: light) { :where(html:not([data-theme="dark"])) .x { … } }   (following the system, unless Dark was picked)
 *   :where(html[data-theme="light"]) .x { … }                                             (Light was picked)
 * `:where()` adds no specificity, so the cascade is exactly what the source says. A `:root` selector becomes `:root:not(…)` / `:root[…]`.
 */
function themeSwitch(): PostcssPlugin {
	const scope = (sel: string, html: string, root: string) => {
		const s = sel.trim();
		return s.startsWith(':root') ? root + s.slice(5) : s === 'html' ? root : `:where(${html}) ${s}`;
	};
	return {
		postcssPlugin: 'stash-theme-switch',
		Once(root) {
			root.walkAtRules('media', (at) => {
				if (!/^\(\s*prefers-color-scheme\s*:\s*light\s*\)$/.test(at.params.trim())) return;
				const forced: Rule[] = [];
				at.each((node) => {
					if (node.type !== 'rule') return;
					const sels = node.selectors;
					forced.push(node.clone({ selectors: sels.map((x) => scope(x, 'html[data-theme="light"]', ':root[data-theme="light"]')) }));
					node.selectors = sels.map((x) => scope(x, 'html:not([data-theme="dark"])', ':root:not([data-theme="dark"])'));
				});
				let prev: ChildNode = at;
				for (const r of forced) {
					at.parent?.insertAfter(prev, r);
					prev = r;
				}
			});
		}
	};
}

export default defineConfig({
	// Settings › About shows it.
	define: { __STASH_VERSION__: JSON.stringify(pkg.version) },
	css: { postcss: { plugins: [themeSwitch()] } },
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) => (filename.split(/[/\\]/).includes('node_modules') ? undefined : true)
			},
			// Static SPA: every route falls back to index.html. The output stays in `dist`: the desktop shell embeds that folder
			// and `stashd serve --ui` serves it.
			adapter: adapter({ pages: 'dist', assets: 'dist', fallback: 'index.html', strict: false })
		})
	],
	server: { host: '127.0.0.1', port: 5199, strictPort: true },
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
