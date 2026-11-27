<!-- Shuffle, previous, play/pause, next, repeat. Used by the bar and the big lyrics. -->
<script lang="ts">
	import { K, withKeys } from './keys';
	import { player } from '../app';
	import Icon from './Icon.svelte';
	import { S } from './state.svelte';

	let { kind = 'bar' }: { kind?: 'bar' | 'fs' } = $props();
	const s = $derived(S.v);
	const nextRepeat = () => player.setRepeat(s.repeat === 'off' ? 'all' : s.repeat === 'all' ? 'one' : 'off');
	const playLabel = $derived(s.listening && s.listenHeld && !s.playing ? 'Back to live' : s.playing ? 'Pause' : 'Play');
	/** Tuned in to a session: skipping (and the queue's order) is the DJ's. */
	const djs = $derived(s.listening ? `${s.listening.host} is playing: skip and seek are theirs` : null);
</script>

<div class="tr tr-{kind}" role="group" aria-label="Playback">
	<button type="button" class="ib" class:on={s.shuffle} aria-pressed={s.shuffle} aria-label="Shuffle" title={djs ?? 'Shuffle'} disabled={!!djs} onclick={() => player.setShuffle(!s.shuffle)}><Icon name="shuffle" /></button>
	<button type="button" class="ib" aria-label="Previous" title={djs ?? withKeys('Previous', K.prev)} disabled={!s.current || !!djs} onclick={() => player.prev()}><Icon name="skip_previous" /></button>
	<button type="button" class="play" class:busy={s.buffering} aria-label={playLabel} title={withKeys(playLabel, K.playPause)} disabled={!s.current} onclick={() => player.toggle()}>
		<Icon name={s.playing ? 'pause' : 'play_arrow'} />
	</button>
	<button type="button" class="ib" aria-label="Next" title={djs ?? withKeys('Next', K.next)} disabled={!s.current || !!djs} onclick={() => player.next()}><Icon name="skip_next" /></button>
	<button type="button" class="ib" class:on={s.repeat !== 'off'} aria-pressed={s.repeat !== 'off'} aria-label={s.repeat === 'one' ? 'Repeat one' : s.repeat === 'all' ? 'Repeat all' : 'Repeat off'} title="Repeat" disabled={!!djs} onclick={nextRepeat}>
		<Icon name={s.repeat === 'one' ? 'repeat_one' : 'repeat'} />
	</button>
</div>
