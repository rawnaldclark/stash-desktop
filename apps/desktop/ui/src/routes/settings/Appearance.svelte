<!-- Settings › Appearance: the theme, pure black, and whether Stash keeps playing in the tray when its window is closed. -->
<script lang="ts">
	import * as core from '../../lib/core';
	import { ui } from '../../lib/ui/state.svelte';
	import { applyAppearance, readAppearance, saveAppearance, type Appearance, type ThemeChoice } from '../../lib/ui/theme';
	import { prefs } from './prefs.svelte';

	let look = $state<Appearance>(readAppearance());
	function setLook(a: Appearance) {
		look = a;
		saveAppearance(a);
		applyAppearance(a);
		ui.theme = a.theme;
	}
	const THEMES: [ThemeChoice, string][] = [
		['dark', 'Dark'],
		['light', 'Light'],
		['system', 'Automatic']
	];

	async function setTray(on: boolean) {
		prefs.tray = on;
		try {
			prefs.tray = (await core.setWindowSettings(on)).tray;
		} catch {
			prefs.tray = !on;
		}
	}
</script>

<div class="set-card">
	<div class="set-row">
		<div class="set-lab"><span class="set-name" id="th-l">Theme</span></div>
		<div class="seg" role="group" aria-labelledby="th-l">
			{#each THEMES as [id, label] (id)}
				<button type="button" class="seg-i" aria-pressed={look.theme === id} onclick={() => setLook({ ...look, theme: id })}>{label}</button>
			{/each}
		</div>
	</div>
	<div class="set-row">
		<div class="set-lab"><span class="set-name" id="bk-l">Pure black</span><span class="set-sub">For OLED screens</span></div>
		<button type="button" class="sw" role="switch" aria-checked={look.black} aria-labelledby="bk-l" onclick={() => setLook({ ...look, black: !look.black })}><i></i></button>
	</div>
	{#if !core.measuring}
		<div class="set-row">
			<div class="set-lab"><span class="set-name" id="tray-l">Keep playing when closed</span><span class="set-sub">Stash stays in the tray</span></div>
			<button type="button" class="sw" role="switch" aria-checked={prefs.tray === true} aria-labelledby="tray-l" disabled={prefs.tray === null} onclick={() => prefs.tray !== null && setTray(!prefs.tray)}><i></i></button>
		</div>
	{/if}
</div>
