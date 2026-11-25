/**
 * Every key the window answers to, in one place: the sheet that lists them (`?`, KeysSheet.svelte), the command
 * palette and the buttons' tooltips all say them from here, so what they say is what the window does.
 * The keys that work in any app (Settings › Shortcuts) are the listener's own and are listed there.
 */
import { BROWSE, keysFor } from '../desktop/library-sections';

/** One key or a combination, as its parts are printed ("Shift", "→"). */
export type Keys = string[];
export type KeyRow = { keys: Keys[]; does: string };
export type KeyGroup = { title: string; rows: KeyRow[] };

/** The keys of the things done most, for tooltips and the palette. */
export const K = {
	playPause: ['Space'],
	next: ['Shift', '→'],
	prev: ['Shift', '←'],
	forward: ['→'],
	back: ['←'],
	louder: ['Shift', '↑'],
	quieter: ['Shift', '↓'],
	search: ['/'],
	palette: ['Ctrl', 'K'],
	keys: ['?'],
	settings: ['Ctrl', ','],
	pageBack: ['Alt', '←'],
	pageForward: ['Alt', '→']
} satisfies Record<string, Keys>;

/** Keys as one piece of text, for a tooltip: "Shift+→". */
export const said = (keys: Keys) => keys.join('+');
/** A label with its keys after it, for a tooltip: "Next (Shift+→)". */
export const withKeys = (label: string, keys: Keys) => `${label} (${said(keys)})`;

export const KEY_GROUPS: KeyGroup[] = [
	{
		title: 'Playing',
		rows: [
			{ keys: [K.playPause], does: 'Play or pause' },
			{ keys: [K.next, K.prev], does: 'Next song, previous song' },
			{ keys: [K.forward, K.back], does: 'Forward or back 5 seconds' },
			{ keys: [K.louder, K.quieter], does: 'Louder, quieter' }
		]
	},
	{
		title: 'Finding',
		rows: [
			{ keys: [K.search], does: 'Search' },
			{ keys: [K.palette], does: 'Find a song, or do anything' },
			{ keys: [K.keys], does: 'These keys' }
		]
	},
	{
		title: 'Going',
		rows: [
			...BROWSE.map((b) => ({ keys: [keysFor(b.id).split('+')], does: `Library › ${b.label}` })),
			{ keys: [K.settings], does: 'Settings' },
			{ keys: [K.pageBack, K.pageForward], does: 'Back, forward' }
		]
	},
	{
		title: 'In Library › Songs',
		rows: [
			{ keys: [['↑'], ['↓']], does: 'Move through the songs' },
			{ keys: [['Shift', '↑'], ['Shift', '↓']], does: 'Pick the songs on the way' },
			{ keys: [['Page Up'], ['Page Down']], does: 'A screen up or down' },
			{ keys: [['Home'], ['End']], does: 'To the top or the end' },
			{ keys: [['Enter']], does: 'Play from the song' }
		]
	},
	{
		title: 'In any list of songs',
		rows: [
			{ keys: [['Ctrl', 'A']], does: 'Pick every song' },
			{ keys: [['Shift', 'F10']], does: 'The song’s menu' },
			{ keys: [['Delete']], does: 'Delete the songs picked' },
			{ keys: [['Esc']], does: 'Let the picked songs go' }
		]
	}
];
