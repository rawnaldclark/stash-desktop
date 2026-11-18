<!-- Settings › Sound: the output and even volume, the equalizer of the output in use (a correction from a file, and two
     tones; each output keeps its own), and exclusive mode under Advanced. -->
<script lang="ts">
	import { TUNINGS } from '../../lib/ui/speed';
	import { onMount } from 'svelte';
	import { player } from '../../lib/app';
	import * as core from '../../lib/core';
	import { trouble } from '../../lib/desktop/folders.svelte';
	import Icon from '../../lib/ui/Icon.svelte';
	import { eqTrouble, outName } from '../../lib/ui/equalizer';
	import { outputSentence } from '../../lib/ui/signal';
	import { S, ui } from '../../lib/ui/state.svelte';
	import { prefs, readOuts } from './prefs.svelte';

	const now = $derived(outputSentence(S.v.signal));
	const RG = [
		['off', 'Off'],
		['track', 'By song'],
		['album', 'By album']
	] as const;

	const outs = $derived(prefs.outs);
	let outTrouble = $state<string | null>(null);
	async function chooseOut(name: string) {
		outTrouble = null;
		try {
			prefs.outs = await core.setDevice(name);
			// The equalizer shown is the new output's at once, not at the next look.
			player.refresh();
		} catch (e) {
			outTrouble = trouble(e);
			void readOuts();
		}
	}
	/** An output's name in the picker. */
	const deviceName = (d: core.Device) => (d.name === 'auto' ? 'System default' : outName(d));

	const eq = $derived(S.v.eq);
	const TONES = [
		['bass', 'Bass'],
		['treble', 'Treble']
	] as const;
	const TILTS = [
		['less', 'Less'],
		['as-is', 'Normal'],
		['more', 'More']
	] as const;
	/** A file that was not brought in, in the reader's words. */
	let eqRefused = $state<string | null>(null);
	let eqPicking = $state(false);
	/** Sound was left (a file's refusal is then said in a toast). */
	let left = false;
	onMount(() => () => (left = true));
	/** One request for each tap. */
	function setEq(patch: core.EqPatch) {
		eqRefused = null;
		player.setEq(patch);
	}
	/** A tone. The core turns the switch on for Less or More; As is leaves the switch as it is. */
	function tone(which: 'bass' | 'treble', t: core.Tilt) {
		setEq(which === 'bass' ? { bass: t } : { treble: t });
	}
	/** "Choose file…": the system's own dialog, then the file is read and its correction put on. */
	async function chooseEqFile() {
		if (eqPicking) return;
		eqPicking = true;
		eqRefused = null;
		try {
			await player.eqFromFile();
		} catch (e) {
			// Said here; when the listener left Sound while the dialog was open, said where they are.
			if (left) ui.toast(trouble(e));
			else eqRefused = trouble(e);
		} finally {
			eqPicking = false;
		}
	}
	/** Which output the equalizer is for, when there is more than one. */
	const eqFor = $derived(outs && outs.devices.length > 1 ? (eq.output.name === 'auto' ? 'For the system default' : `For ${outName(eq.output)}`) : null);
	/** What the equalizer does to the sound now, when that is worth a line. */
	const eqSays = $derived(eq.reopen_at_pause ? 'Bit-perfect again after the next pause' : eq.on && eq.softer_db >= 0.05 ? `Plays ${eq.softer_db.toFixed(1)} dB quieter to avoid distortion` : null);
	const eqAlert = $derived(eqTrouble(eq.trouble));
</script>

{#if !S.v.available}
	<div class="set-card">
		<div class="set-na" role="status">
			<b>No sound</b>
			<span>{S.v.reason ?? 'The player isn’t available'}</span>
		</div>
	</div>
{:else}
	<div class="set-card">
		{#if outs && outs.devices.length > 1}
			<div class="set-row">
				<div class="set-lab">
					<label class="set-name" for="out-pick">Output</label>
					{#if outs.missing}<span class="set-sub" role="status">{outs.missing.description || 'Your output'} isn’t connected. Using the default.</span>{/if}
					{#if outTrouble}<span class="set-sub set-trouble" role="alert">{outTrouble}</span>{/if}
				</div>
				{#key outs}
					<select id="out-pick" class="dev-input set-select" onchange={(e) => chooseOut(e.currentTarget.value)}>
						{#each outs.devices as d (d.name)}<option value={d.name} selected={outs.device === d.name}>{deviceName(d)}</option>{/each}
					</select>
				{/key}
			</div>
		{/if}
		<div class="set-row">
			<div class="set-lab"><span class="set-name" id="rg-l">Even out volume</span></div>
			<div class="seg" role="group" aria-labelledby="rg-l">
				{#each RG as [mode, label] (mode)}
					<button type="button" class="seg-i" aria-pressed={S.v.replaygain === mode} onclick={() => player.setReplaygain(mode)}>{label}</button>
				{/each}
			</div>
		</div>
		<!-- Tuning (Android's Settings › Audio): the A songs are heard at; any other value is set from the speed button. -->
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="tune-l">Tuning</span>
				<span class="set-sub">{S.v.tuning === 440 ? 'As recorded (A = 440 Hz).' : S.v.tuning < 440 ? `A = ${S.v.tuning} Hz: a little lower in pitch. Same speed.` : `A = ${S.v.tuning} Hz: a little higher in pitch. Same speed.`}</span>
			</div>
			<div class="seg" role="group" aria-labelledby="tune-l">
				{#each TUNINGS as hz (hz)}
					<button type="button" class="seg-i" aria-pressed={S.v.tuning === hz} onclick={() => player.setTuning(hz)}>{hz === 440 ? 'Standard' : hz}</button>
				{/each}
			</div>
		</div>
	</div>

	<div class="set-card">
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="eq-l">Equalizer</span>
				{#if eqFor}<span class="set-sub">{eqFor}</span>{/if}
			</div>
			<button type="button" class="sw" role="switch" aria-checked={eq.on} aria-labelledby="eq-l" onclick={() => setEq({ on: !eq.on })}><i></i></button>
		</div>
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="corr-l">Headphone correction</span>
				{#if eq.correction}
					<span class="set-sub eq-name" title={eq.correction.summary}>{eq.correction.name}</span>
				{:else}
					<span class="set-sub">From autoeq.app, in Equalizer APO format</span>
				{/if}
				{#if eqRefused}<span class="set-sub set-trouble" role="alert">{eqRefused}</span>{/if}
			</div>
			<div class="set-ctl" role="group" aria-labelledby="corr-l">
				{#if eq.correction}
					<button type="button" class="btn ghost" disabled={eqPicking} onclick={chooseEqFile}>Change…</button>
					<button type="button" class="btn ghost" onclick={() => setEq({ correction: null })}>Remove</button>
				{:else}
					<button type="button" class="btn ghost" disabled={eqPicking} onclick={chooseEqFile}>Choose file…</button>
				{/if}
			</div>
		</div>
		{#each TONES as [which, name] (which)}
			<div class="set-row">
				<div class="set-lab"><span class="set-name" id="{which}-l">{name}</span></div>
				<div class="seg" class:eq-off={!eq.on} role="group" aria-labelledby="{which}-l">
					{#each TILTS as [t, label] (t)}
						<button type="button" class="seg-i" aria-pressed={eq[which] === t} onclick={() => tone(which, t)}>{label}</button>
					{/each}
				</div>
			</div>
		{/each}
		{#if eqSays || eqAlert}
			<div class="set-row set-note">
				{#if eqSays}<p class="set-sub" role="status">{eqSays}</p>{/if}
				{#if eqAlert}<p class="set-sub set-trouble" role="alert">{eqAlert}</p>{/if}
			</div>
		{/if}
	</div>

	<details class="set-card set-more">
		<summary><span>Advanced</span><Icon name="expand_more" class="ic set-chev" /></summary>
		<div class="set-row">
			<div class="set-lab">
				<span class="set-name" id="ex-l">Exclusive mode</span>
				<span class="set-sub">Bit-perfect output; other apps go silent</span>
				{#if now}<span class="set-sub" role="status">{now}</span>{/if}
			</div>
			<button type="button" class="sw" role="switch" aria-checked={S.v.exclusive} aria-labelledby="ex-l" onclick={() => player.setExclusive(!S.v.exclusive)}><i></i></button>
		</div>
	</details>
{/if}
