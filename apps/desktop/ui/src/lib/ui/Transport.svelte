<!-- Shuffle, previous, play/pause, next, repeat. Used by the bar and the big lyrics. -->
<script lang="ts">
	import { player } from '../app';
	import Icon from './Icon.svelte';
	import { S } from './state.svelte';

	let { kind = 'bar' }: { kind?: 'bar' | 'fs' } = $props();
	const s = $derived(S.v);
	const nextRepeat = () => player.setRepeat(s.repeat === 'off' ? 'all' : s.repeat === 'all' ? 'one' : 'off');
	const playLabel = $derived(s.playing ? 'Pause' : 'Play');
</script>

<div class="tr tr-{kind}" role="group" aria-label="Playback">
	<button type="button" class="ib" class:on={s.shuffle} aria-pressed={s.shuffle} aria-label="Shuffle" title="Shuffle" onclick={() => player.setShuffle(!s.shuffle)}><Icon name="shuffle" /></button>
	<button type="button" class="ib" aria-label="Previous" title="Previous" disabled={!s.current} onclick={() => player.prev()}><Icon name="skip_previous" /></button>
	<button type="button" class="play" class:busy={s.buffering} aria-label={playLabel} title={playLabel} disabled={!s.current} onclick={() => player.toggle()}>
		<Icon name={s.playing ? 'pause' : 'play_arrow'} />
	</button>
	<button type="button" class="ib" aria-label="Next" title="Next" disabled={!s.current} onclick={() => player.next()}><Icon name="skip_next" /></button>
	<button type="button" class="ib" class:on={s.repeat !== 'off'} aria-pressed={s.repeat !== 'off'} aria-label={s.repeat === 'one' ? 'Repeat one' : s.repeat === 'all' ? 'Repeat all' : 'Repeat off'} title="Repeat" onclick={nextRepeat}>
		<Icon name={s.repeat === 'one' ? 'repeat_one' : 'repeat'} />
	</button>
</div>
