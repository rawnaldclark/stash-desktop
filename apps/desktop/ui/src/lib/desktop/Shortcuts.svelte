<!-- Settings › Shortcuts: keys that work in any app (play or pause, skip, the volume, like the song playing, show
     Stash). The core keeps them and the desktop app holds them system-wide; in a browser there are none, and the
     section does not show (`shown` tells the page, so its list of groups leaves it out too). -->
<script lang="ts">
	import { onMount } from 'svelte';
	import * as core from '../core';
	import { Refused } from '../core';

	let { shown = $bindable(false) }: { shown?: boolean } = $props();

	const LABELS: Record<string, string> = {
		toggle: 'Play or pause',
		next: 'Next song',
		previous: 'Previous song',
		volume_up: 'Volume up',
		volume_down: 'Volume down',
		like: 'Like current song',
		show: 'Show Stash'
	};
	const mac = typeof navigator !== 'undefined' && /Mac/.test(navigator.userAgent);
	const MODIFIERS: Record<string, string> = { Ctrl: mac ? 'Control' : 'Ctrl', Alt: mac ? 'Option' : 'Alt', Shift: 'Shift', Super: mac ? 'Command' : 'Win' };
	const NEEDS = mac ? 'Control, Option or Command' : 'Ctrl, Alt or the Windows key';
	/** Keys whose name in the page is not the one on the key. */
	const KEY_NAMES: Record<string, string> = {
		ArrowUp: 'Up',
		ArrowDown: 'Down',
		ArrowLeft: 'Left',
		ArrowRight: 'Right',
		PageUp: 'Page Up',
		PageDown: 'Page Down',
		Backquote: '`',
		Minus: '-',
		Equal: '=',
		BracketLeft: '[',
		BracketRight: ']',
		Backslash: '\\',
		Semicolon: ';',
		Quote: "'",
		Comma: ',',
		Period: '.',
		Slash: '/',
		NumpadAdd: 'Num +',
		NumpadSubtract: 'Num -',
		NumpadMultiply: 'Num *',
		NumpadDivide: 'Num /',
		NumpadDecimal: 'Num .',
		NumpadEnter: 'Num Enter'
	};
	/** The keys that only change others. */
	const ALONE = new Set(['Control', 'Alt', 'Shift', 'Meta', 'AltGraph', 'OS']);

	let s = $state<core.Shortcuts | null>(null);
	/** The action whose keys are being picked. */
	let picking = $state<string | null>(null);
	/** The modifiers held while picking, as they are pressed. */
	let held = $state('');
	let wrong = $state<Record<string, string>>({});
	let busy = $state(false);

	onMount(() => {
		core.shortcuts().then(
			(v) => {
				s = v;
				shown = v.available;
			},
			() => {}
		);
		// Leaving Settings in the middle of picking: the keys are held again.
		return () => {
			if (picking) void core.setShortcuts({ paused: false }).catch(() => {});
		};
	});

	function keyName(code: string): string {
		if (KEY_NAMES[code]) return KEY_NAMES[code];
		if (/^Key[A-Z]$/.test(code)) return code.slice(3);
		if (/^Digit\d$/.test(code)) return code.slice(5);
		if (/^Numpad\d$/.test(code)) return `Num ${code.slice(6)}`;
		return code;
	}
	/** Keys as the listener reads them: "Ctrl + Alt + Page Down". */
	function say(keys: string): string {
		if (!keys) return 'No keys';
		const words = keys.split('+');
		const key = words.pop() ?? '';
		return [...words.map((m) => MODIFIERS[m] ?? m), keyName(key)].join(' + ');
	}
	function modifiers(e: KeyboardEvent): string[] {
		const held: string[] = [];
		if (e.ctrlKey) held.push('Ctrl');
		if (e.altKey) held.push('Alt');
		if (e.shiftKey) held.push('Shift');
		if (e.metaKey) held.push('Super');
		return held;
	}
	/** As the core has it: keys alone, or with Shift only, would stop typing in every other app. F13 to F24 type nothing. */
	function usable(held: string[], code: string): boolean {
		const f = /^F(\d+)$/.exec(code);
		const beyondTyping = !!f && +f[1] >= 13 && +f[1] <= 24;
		return held.includes('Ctrl') || held.includes('Alt') || held.includes('Super') || beyondTyping;
	}

	async function change(what: Parameters<typeof core.setShortcuts>[0], action?: string) {
		busy = true;
		try {
			s = await core.setShortcuts(what);
		} catch (e) {
			if (action) wrong = { ...wrong, [action]: e instanceof Refused ? e.message : 'Couldn’t save those keys' };
			s = await core.shortcuts().catch(() => s);
		} finally {
			busy = false;
		}
	}
	async function pick(action: string) {
		wrong = { ...wrong, [action]: '' };
		picking = action;
		held = '';
		// Keys held system-wide never reach the window: they are let go of while new ones are picked.
		await change({ paused: true });
	}
	/** Ends picking: with the keys picked ("" for none), or leaving them as they were. */
	async function done(keys?: string) {
		const action = picking;
		picking = null;
		held = '';
		if (!action) return;
		await change(keys === undefined ? { paused: false } : { paused: false, keys: { [action]: keys } }, action);
	}
	function down(e: KeyboardEvent, action: string) {
		if (picking !== action) return;
		e.preventDefault();
		e.stopPropagation();
		const mods = modifiers(e);
		if (ALONE.has(e.key)) {
			held = mods.length ? `${mods.map((m) => MODIFIERS[m]).join(' + ')} + …` : '';
			return;
		}
		if (!mods.length && e.key === 'Escape') return void done();
		if (!mods.length && (e.key === 'Backspace' || e.key === 'Delete')) return void done('');
		if (!usable(mods, e.code)) {
			held = '';
			wrong = { ...wrong, [action]: `Add ${NEEDS}` };
			return;
		}
		const keys = [...mods, e.code].join('+');
		const other = Object.entries(s?.keys ?? {}).find(([it, theirs]) => it !== action && theirs.toLowerCase() === keys.toLowerCase());
		if (other) {
			wrong = { ...wrong, [action]: `${say(keys)} is already used for ${(LABELS[other[0]] ?? other[0]).toLowerCase()}` };
			return;
		}
		void done(keys);
	}
	function up(e: KeyboardEvent, action: string) {
		if (picking !== action) return;
		e.preventDefault();
		e.stopPropagation();
		const mods = modifiers(e);
		held = mods.length ? `${mods.map((m) => MODIFIERS[m]).join(' + ')} + …` : '';
	}
</script>

{#if s?.available}
	<section class="set-sec" id="shortcuts" aria-labelledby="h-shortcuts">
		<h2 class="set-g" id="h-shortcuts" tabindex="-1">Shortcuts</h2>
		<div class="set-card">
			<div class="set-row">
				<div class="set-lab">
					<span class="set-name" id="sk-l">Global shortcuts</span>
					<span class="set-sub">Work while you use other apps</span>
				</div>
				<button type="button" class="sw" role="switch" aria-checked={s.on} aria-labelledby="sk-l" disabled={busy} onclick={() => s && void change({ on: !s.on })}><i></i></button>
			</div>
			{#each s.actions as action (action)}
				<div class="set-row sk-row" class:sk-off={!s.on}>
					<div class="set-lab">
						<span class="set-name" id="sk-{action}">{LABELS[action] ?? action}</span>
						{#if picking === action}
							<span class="set-sub" role="status">Esc to cancel, Backspace to clear</span>
						{/if}
						{#if s.on && s.trouble[action] && picking !== action}<span class="set-sub set-trouble">{s.trouble[action]}. Pick other keys.</span>{/if}
						{#if wrong[action]}<span class="set-sub set-trouble" role="alert">{wrong[action]}</span>{/if}
					</div>
					<button
						type="button"
						class="btn ghost sk-keys"
						class:picking={picking === action}
						class:none={!s.keys[action]}
						aria-labelledby="sk-{action} sk-k-{action}"
						onclick={() => void (picking === action ? done() : pick(action))}
						onkeydown={(e) => down(e, action)}
						onkeyup={(e) => up(e, action)}
						onblur={() => picking === action && void done()}
					><span id="sk-k-{action}">{picking === action ? held || 'Press the keys…' : say(s.keys[action])}</span></button>
				</div>
			{/each}
		</div>
	</section>
{/if}

<style>
	.sk-row.sk-off .set-name { opacity: 0.6; }
	.sk-keys { min-width: 168px; flex: none; font-variant-numeric: tabular-nums; }
	.sk-keys.none { color: var(--wp-text-2, var(--wp-text)); font-weight: 500; }
	.sk-keys.picking { border-color: var(--acc); box-shadow: 0 0 0 2px var(--acc-wash); }
</style>
