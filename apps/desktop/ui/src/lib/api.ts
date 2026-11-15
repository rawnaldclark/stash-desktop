// The window's only way to the library: the core's local API. Served by the core itself, the API is on this page's
// own origin; in the desktop window the core's address is handed in as `?core=http://127.0.0.1:<port>`.

export type TrackRow = {
  id: number;
  title: string;
  artist: string;
  album: string | null;
  duration_ms: number;
  codec: string | null;
  sample_rate: number | null;
  bit_depth: number | null;
  origin: "file" | "addon";
  sort_key: string;
};

export type Counts = { tracks: number; albums: number; artists: number };

const core = new URLSearchParams(location.search).get("core") ?? "";

async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`${core}${path}`, { signal });
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  return (await response.json()) as T;
}

export const counts = () => get<Counts>("/api/counts");

export const tracksAt = (offset: number, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/tracks?offset=${offset}&limit=${limit}`, signal).then((r) => r.rows);

export const search = (query: string, limit: number, signal?: AbortSignal) =>
  get<{ rows: TrackRow[] }>(`/api/search?q=${encodeURIComponent(query)}&limit=${limit}`, signal).then((r) => r.rows);

export async function postMeasurement(result: unknown): Promise<void> {
  await fetch(`${core}/api/bench-result`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(result) });
}
