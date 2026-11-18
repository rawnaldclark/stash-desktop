<!-- Settings › Playback: autoplay, similar music from Last.fm (radio, and autoplay that knows what is like what), lyrics
     from LRCLIB, and the songs and artists not to play. -->
<script lang="ts">
	import * as core from '../../lib/core';
	import Blocked from '../../lib/desktop/Blocked.svelte';
	import { R } from '../../lib/desktop/radio.svelte';
	import { ui } from '../../lib/ui/state.svelte';
	import { prefs } from './prefs.svelte';

	let similarBusy = $state(false);
	async function setSimilar(on: boolean) {
		similarBusy = true;
		try {
			await R.set(on);
		} catch (e) {
			ui.toast(core.said(e));
		} finally {
			similarBusy = false;
		}
	}

	async function setAutoplay(on: boolean) {
		prefs.autoplay = on;
		try {
			prefs.autoplay = (await core.setAutoplay(on)).on;
		} catch {
			prefs.autoplay = !on;
		}
	}

	async function setLrclib(on: boolean) {
		prefs.lrclib = on;
		try {
			prefs.lrclib = (await core.setLyricsSettings(on)).lrclib;
		} catch {
			prefs.lrclib = !on;
		}
	}
</script>

<div class="set-card">
	<div class="set-row">
		<div class="set-lab">
			<span class="set-name" id="ap-l">Autoplay</span>
			<span class="set-sub">Plays similar songs when the queue ends</span>
		</div>
		<button type="button" class="sw" role="switch" aria-checked={prefs.autoplay === true} aria-labelledby="ap-l" disabled={prefs.autoplay === null} onclick={() => prefs.autoplay !== null && setAutoplay(!prefs.autoplay)}><i></i></button>
	</div>
	{#if R.available}
		<div class="set-row" id="similar">
			<div class="set-lab">
				<span class="set-name" id="lfm-l">Similar music</span>
				<span class="set-sub">Last.fm suggestions for radio and autoplay</span>
			</div>
			<button type="button" class="sw" role="switch" aria-checked={R.on} aria-labelledby="lfm-l" disabled={similarBusy} onclick={() => void setSimilar(!R.on)}><i></i></button>
		</div>
	{/if}
	<div class="set-row" id="lyrics">
		<div class="set-lab">
			<span class="set-name" id="lrc-l">Find lyrics online</span>
			<span class="set-sub">From LRCLIB when a song has none</span>
		</div>
		<button type="button" class="sw" role="switch" aria-checked={prefs.lrclib === true} aria-labelledby="lrc-l" disabled={prefs.lrclib === null} onclick={() => prefs.lrclib !== null && setLrclib(!prefs.lrclib)}><i></i></button>
	</div>
</div>
<Blocked />
