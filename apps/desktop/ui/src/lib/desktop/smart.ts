// Smart playlists in the window: the rules' fields and how each is compared, in words; the orders; the starting points
// offered for a new one; and the rules said back as a line. The core checks and runs them (`crates/stash-core/src/smart.rs`).
import type { SmartField, SmartOp, SmartOrder, SmartRule, SmartRules } from '../core';

/** What a rule's value is, as the editor asks for it. Length is asked in minutes and kept in seconds. */
export type ValueKind = 'text' | 'number' | 'minutes' | 'days' | 'often' | 'quality' | 'none';
export type FieldDef = { id: SmartField; label: string; ops: [SmartOp, string][]; kind: ValueKind; unit?: string };

const TEXT_OPS: [SmartOp, string][] = [
	['contains', 'contains'],
	['not_contains', 'does not contain'],
	['is', 'is'],
	['is_not', 'is not'],
	['starts_with', 'starts with']
];

export const FIELDS: FieldDef[] = [
	{ id: 'artist', label: 'Artist', ops: TEXT_OPS, kind: 'text' },
	{ id: 'title', label: 'Title', ops: TEXT_OPS, kind: 'text' },
	{ id: 'album', label: 'Album', ops: TEXT_OPS, kind: 'text' },
	{ id: 'liked', label: 'Liked', ops: [['is', 'yes'], ['is_not', 'no']], kind: 'none' },
	{ id: 'plays', label: 'Plays', ops: [['at_least', 'at least'], ['below', 'fewer than'], ['is', 'exactly']], kind: 'number', unit: 'times' },
	{ id: 'played_often', label: 'Played', ops: [['at_least', 'at least']], kind: 'often' },
	{ id: 'last_played', label: 'Last played', ops: [['within', 'in the last'], ['not_within', 'not in the last']], kind: 'days', unit: 'days' },
	{ id: 'added', label: 'Added', ops: [['within', 'in the last'], ['not_within', 'not in the last']], kind: 'days', unit: 'days' },
	{ id: 'year', label: 'Year', ops: [['is', 'is'], ['is_not', 'is not'], ['at_least', 'is at least'], ['below', 'is before']], kind: 'number' },
	{ id: 'length', label: 'Length', ops: [['at_least', 'is at least'], ['below', 'is shorter than']], kind: 'minutes', unit: 'min' },
	{ id: 'quality', label: 'Quality', ops: [['at_least', 'is at least'], ['is', 'is']], kind: 'quality' },
	{ id: 'here', label: 'On this computer', ops: [['is', 'yes'], ['is_not', 'no']], kind: 'none' }
];

export const QUALITIES: [string, string][] = [
	['lossy', 'Lossy'],
	['lossless', 'Lossless'],
	['hires', 'Hi-res']
];

export const ORDERS: [SmartOrder, string][] = [
	['added', 'Newest added first'],
	['most_played', 'Most played first'],
	['recently_played', 'Played most recently first'],
	['least_recently_played', 'Not played for the longest first'],
	['title', 'By title'],
	['artist', 'By artist'],
	['album', 'By album'],
	['year', 'Newest year first'],
	['random', 'Shuffled, a new pick each day']
];

export const fieldOf = (id: SmartField): FieldDef => FIELDS.find((f) => f.id === id) ?? FIELDS[0];

/** A new rule for `field`: its first way of comparing, and a value to start from. */
export function ruleFor(field: SmartField): SmartRule {
	const f = fieldOf(field);
	const op = f.ops[0][0];
	switch (f.kind) {
		case 'text':
			return { field, op, value: '' };
		case 'number':
			return { field, op, value: field === 'year' ? 2000 : 1 };
		case 'minutes':
			return { field, op, value: 300 };
		case 'days':
			return { field, op, value: 30 };
		case 'often':
			return { field, op, value: 3, days: 30 };
		case 'quality':
			return { field, op, value: 'lossless' };
		case 'none':
			return { field, op };
	}
}

/** Where a new smart playlist can start from. */
export const PRESETS: { name: string; rules: SmartRules }[] = [
	{ name: 'Recently added', rules: { any: false, rules: [{ field: 'added', op: 'within', value: 30 }], order: 'added' } },
	{ name: 'Most played', rules: { any: false, rules: [{ field: 'plays', op: 'at_least', value: 1 }], order: 'most_played', limit: 50 } },
	{ name: 'On repeat', rules: { any: false, rules: [{ field: 'played_often', op: 'at_least', value: 3, days: 30 }], order: 'most_played' } },
	{
		name: 'Forgotten favourites',
		rules: { any: false, rules: [{ field: 'liked', op: 'is' }, { field: 'last_played', op: 'not_within', value: 90 }], order: 'least_recently_played', limit: 50 }
	},
	{ name: 'Never played', rules: { any: false, rules: [{ field: 'plays', op: 'is', value: 0 }], order: 'added' } },
	{ name: 'Lossless', rules: { any: false, rules: [{ field: 'quality', op: 'at_least', value: 'lossless' }], order: 'artist' } },
	{ name: 'Daily mix', rules: { any: false, rules: [{ field: 'liked', op: 'is' }], order: 'random', limit: 50 } }
];

const minutes = (seconds: unknown) => Math.round((Number(seconds) || 0) / 60);

/** One rule in words: "Artist contains “lanterns”", "Not liked", "Played at least 3 times in the last 30 days". */
export function sayRule(rule: SmartRule): string {
	const f = fieldOf(rule.field);
	const op = f.ops.find(([o]) => o === rule.op)?.[1] ?? rule.op;
	switch (f.kind) {
		case 'text':
			return `${f.label} ${op} “${rule.value ?? ''}”`;
		case 'number':
			return `${f.label} ${op} ${rule.value}${f.unit ? ` ${f.unit}` : ''}`;
		case 'minutes':
			return `${f.label} ${op} ${minutes(rule.value)} min`;
		case 'days':
			return `${f.label} ${op} ${rule.value} days`;
		case 'often':
			return `Played at least ${rule.value} times in the last ${rule.days} days`;
		case 'quality':
			return `${f.label} ${op} ${QUALITIES.find(([q]) => q === rule.value)?.[1] ?? rule.value}`;
		case 'none':
			return rule.op === 'is' ? f.label : `Not ${f.label.toLowerCase()}`;
	}
}

/** The rules as a line: "Liked · Last played not in the last 90 days · not played for the longest first · 50 at most". */
export function describe(r: SmartRules): string {
	const said = r.rules.map(sayRule);
	const what = said.length ? said.join(r.any ? ', or ' : ' · ') : 'Every song';
	const order = (ORDERS.find(([o]) => o === (r.order ?? 'added'))?.[1] ?? '').toLowerCase();
	return [what, order, r.limit ? `${r.limit} at most` : ''].filter(Boolean).join(' · ');
}
