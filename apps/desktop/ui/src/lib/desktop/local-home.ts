/** What Home says of the files on this computer (LocalHome.svelte), apart from the screen so it is tested. */
import type { LocalStats } from '../core';

const count = (n: number, one: string, many: string) => `${n.toLocaleString()} ${n === 1 ? one : many}`;

/** How long it would take to play everything: "3 days 4 hours", "5 hours 12 minutes", "42 minutes". */
export function longTime(ms: number): string {
	const minutes = Math.round(ms / 60_000);
	if (minutes < 1) return 'under a minute';
	const days = Math.floor(minutes / 1440);
	const hours = Math.floor((minutes % 1440) / 60);
	const mins = minutes % 60;
	if (days) return hours ? `${count(days, 'day', 'days')} ${count(hours, 'hour', 'hours')}` : count(days, 'day', 'days');
	if (hours) return mins ? `${count(hours, 'hour', 'hours')} ${count(mins, 'minute', 'minutes')}` : count(hours, 'hour', 'hours');
	return count(mins, 'minute', 'minutes');
}

/** A size on disk: "38.5 GB", "720 MB". */
export function diskSize(bytes: number): string {
	if (bytes >= 1_073_741_824) return `${(bytes / 1_073_741_824).toFixed(bytes >= 107_374_182_400 ? 0 : 1)} GB`;
	return `${Math.max(1, Math.round(bytes / 1_048_576))} MB`;
}

/** The numbers under Home's heading, each with what it counts. */
export function statTiles(s: LocalStats): { value: string; label: string; href?: string }[] {
	return [
		{ value: s.songs.toLocaleString(), label: s.songs === 1 ? 'song' : 'songs', href: '/library?tab=songs' },
		{ value: s.albums.toLocaleString(), label: s.albums === 1 ? 'album' : 'albums', href: '/library?tab=albums' },
		{ value: s.artists.toLocaleString(), label: s.artists === 1 ? 'artist' : 'artists', href: '/library?tab=artists' },
		{ value: s.genres.toLocaleString(), label: s.genres === 1 ? 'genre' : 'genres', href: '/library?tab=genres' },
		{ value: longTime(s.duration_ms), label: 'of music' },
		{ value: diskSize(s.bytes), label: 'on disk' }
	];
}
