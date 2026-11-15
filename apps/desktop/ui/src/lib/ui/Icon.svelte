<!-- One Material Icons Round glyph (24x24 path). Decorative unless `label` is given. -->
<script lang="ts">
	import { ICONS, type IconName } from './icons';

	let { name, class: cls = 'ic', label }: { name: IconName; class?: string; label?: string } = $props();
</script>

<svg class={cls} viewBox="0 0 24 24" role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'} focusable="false"><path d={ICONS[name]} /></svg>
