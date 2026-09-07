/**
 * Tiny module-level queue that survives view unmount. The Retrospective
 * view fires-and-forgets via `start()`; the call keeps running in Rust
 * even if the user navigates away. When the backend emits
 * `retro-generated` / `retro-failed`, App.tsx's listener calls
 * `finish()` and any subscribed view updates its UI.
 *
 * Not a state library — just enough to share "what's currently being
 * generated" across components that mount/unmount independently.
 */
import { invoke } from "@tauri-apps/api/core";

const inFlight = new Set<string>(); // dungeon_id strings
const listeners = new Set<(snapshot: ReadonlySet<string>) => void>();

function notify() {
  const snap: ReadonlySet<string> = new Set(inFlight);
  listeners.forEach((l) => l(snap));
}

export const retroQueue = {
  /** Mark a dungeon as currently generating and fire the Tauri call.
   *  The promise is intentionally not returned — completion is signalled
   *  via the global `retro-generated` / `retro-failed` events that
   *  App.tsx forwards to `finish()`. */
  start(dungeonId: string, model: "sonnet" | "opus") {
    if (inFlight.has(dungeonId)) return;
    inFlight.add(dungeonId);
    notify();
    // .finally — clear the in-flight flag regardless of resolve/reject.
    // The backend also emits `retro-generated`/`retro-failed` which flips
    // the queue too, but relying on that alone left a window where the
    // event listener (in App.tsx) hadn't registered yet for an unmounted
    // ancestor case, so the busy indicator stayed forever. Belt-and-
    // suspenders: clear here when the invoke promise settles.
    invoke("retrospective_generate", { dungeonId, model }).finally(() => {
      if (inFlight.delete(dungeonId)) notify();
    });
  },
  /** Mark a dungeon as no longer generating. Called by App.tsx event
   *  listener when the backend signals completion. */
  finish(dungeonId: string) {
    if (inFlight.delete(dungeonId)) notify();
  },
  has(dungeonId: string): boolean {
    return inFlight.has(dungeonId);
  },
  size(): number {
    return inFlight.size;
  },
  /** Subscribe to changes. The callback is invoked immediately with the
   *  current snapshot, then on every mutation. Returns an unsubscribe. */
  subscribe(cb: (snapshot: ReadonlySet<string>) => void): () => void {
    listeners.add(cb);
    cb(new Set(inFlight));
    return () => {
      listeners.delete(cb);
    };
  },
};
