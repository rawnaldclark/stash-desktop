/**
 * The music folders on this computer, and the scan that reads them into the library (Settings › Your library). Kept outside the page,
 * so a scan that is running is still followed to its end when the listener looks at something else meanwhile.
 */
import { library } from '../app';
import * as core from '../core';

const whole = new Intl.NumberFormat();
export const many = (n: number, one: string, more: string) => `${whole.format(n)} ${n === 1 ? one : more}`;
/** The last part of a path: the folder's own name. */
export const lastPart = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
/** What went wrong, as a sentence. */
export const trouble = (e: unknown) => {
	const text = core.said(e);
	return /[.!?]$/.test(text) ? text : `${text}.`;
};

export type Notice = { text: string; trouble: boolean };

class Folders {
	/** The music folders the listener added. */
	list = $state<string[]>([]);
	/** The scan that is running, as the core last told it; null when none is. */
	scan = $state.raw<core.Scanning['scan'] | null>(null);
	/** What the last scan said. */
	notice = $state<Notice | null>(null);
	private following = false;
	private asked = false;

	/** Reads the folders once; a scan that was running when the window opened is followed to its end. */
	async init(): Promise<void> {
		if (this.asked) return;
		this.asked = true;
		const state = await core.scanning().catch(() => null);
		if (!state) {
			this.asked = false;
			return;
		}
		this.list = state.folders;
		if (state.scan.running) void this.follow(state);
	}

	/** Follows a scan until it ends, then takes in what it changed and says so. */
	private async follow(started: core.Scanning): Promise<void> {
		this.list = started.folders;
		this.scan = started.scan;
		if (this.following) return;
		this.following = true;
		while (this.scan?.running) {
			await new Promise((resolve) => setTimeout(resolve, 400));
			const state = await core.scanning().catch(() => null);
			if (!state) continue;
			this.list = state.folders;
			this.scan = state.scan;
		}
		this.following = false;
		const ended = this.scan;
		this.scan = null;
		await library.reload();
		if (ended?.error) {
			this.notice = { text: `The scan stopped: ${ended.error}.`, trouble: true };
		} else if (ended?.report) {
			const r = ended.report;
			const parts = [
				many(r.added, 'new song', 'new songs'),
				r.changed ? `${whole.format(r.changed)} read again` : '',
				r.missing ? `${whole.format(r.missing)} no longer there` : '',
				r.unreadable ? `${many(r.unreadable, 'file', 'files')} that could not be read` : ''
			].filter((p) => p !== '');
			const nothing = r.added === 0 && parts.length === 1;
			this.notice = { text: `Scanned ${many(r.files, 'file', 'files')}: ${nothing ? 'nothing has changed' : parts.join(', ')}.`, trouble: false };
		}
	}

	/** Asks for a folder with the system's own dialog, then scans it and remembers it. */
	async add(): Promise<void> {
		try {
			const folder = await core.pickFolder();
			if (folder) await this.follow(await core.scan(folder));
		} catch (e) {
			this.notice = { text: trouble(e), trouble: true };
		}
	}

	/** Scans the remembered folders again. */
	async again(): Promise<void> {
		try {
			await this.follow(await core.scan());
		} catch (e) {
			this.notice = { text: trouble(e), trouble: true };
		}
	}
}

export const folders = new Folders();
