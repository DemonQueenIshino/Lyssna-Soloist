import { describe, it, expect } from "vitest";
import {
  reducer,
  advanceDetails,
  emptyState,
  activeLines,
  endsOnRight,
  progress,
  position,
  nextToggle,
  sessionScreen,
  type AppState,
  type ToggleIntent,
} from "./state";
import type { Track } from "./types";
import { PlaybackClock } from "./playback-clock";
const track: Track = {
  id: "song",
  uri: "spotify:track:song",
  name: "Song",
  artists: [],
  album: { name: "Album", images: [] },
  duration_ms: 10000,
  is_local: false,
};
describe("credits following the last singer", () => {
  const line = (opposite: boolean, background = false) => ({
    opposite,
    background,
  });
  it("follows the last lead line, ignoring background vocals", () => {
    expect(endsOnRight([line(false), line(true)], false)).toBe(true);
    expect(endsOnRight([line(true), line(false)], false)).toBe(false);
    expect(endsOnRight([line(true), line(false, true)], false)).toBe(true);
  });
  it("swaps sides when duets are mirrored", () => {
    expect(endsOnRight([line(true)], true)).toBe(false);
    expect(endsOnRight([line(false)], true)).toBe(true);
    expect(endsOnRight([], true)).toBe(false);
  });
});
describe("asynchronous playback and browsing", () => {
  it("keeps all music search categories together and rejects old responses", () => {
    const artist = {
      id: "artist",
      name: "Artist",
      uri: "spotify:artist:artist",
    };
    const album = {
      id: "album",
      uri: "spotify:album:album",
      name: "Album",
      images: [],
    };
    const state = {
      ...emptyState,
      profile: { id: "listener" },
      route: "search:music",
      generation: 5,
      loading: true,
    };
    const event = {
      type: "search_results" as const,
      account: "listener",
      key: state.route,
      generation: 5,
      message: "",
      results: {
        tracks: [track],
        artists: [artist],
        albums: [album],
        playlists: [],
        top_results: [artist.uri],
      },
    };
    expect(reducer(state, { ...event, account: "other" })).toBe(state);
    expect(reducer(state, { ...event, key: "search:old" })).toBe(state);
    expect(reducer(state, { ...event, generation: 4 })).toBe(state);
    expect(reducer(emptyState, event)).toBe(emptyState);
    const loaded = reducer(state, event);
    expect(loaded.loading).toBe(false);
    expect(loaded.tracks).toEqual([track]);
    expect(loaded.search_artists).toEqual([artist]);
    const other = reducer(loaded, {
      type: "navigate",
      key: "liked",
      generation: 6,
    });
    expect(other.search_artists).toEqual([]);
    const back = reducer(other, {
      type: "navigate",
      key: state.route,
      generation: 7,
    });
    expect(back.search_artists).toEqual([artist]);
    expect(back.search_albums).toEqual([album]);
    expect(back.search_top_results).toEqual([artist.uri]);
    expect(reducer(back, { type: "signed_out" }).search_artists).toEqual([]);
  });
  it("rejects playlists from old searches, refreshes and signed-out accounts", () => {
    const playlist = {
      id: "public",
      name: "Public",
      images: [],
      owner: { id: "listener" },
      collaborative: false,
    };
    const state = {
      ...emptyState,
      profile: { id: "listener" },
      route: "search:workout",
      generation: 2,
      search_playlists_loading: true,
    };
    const event = {
      type: "search_playlists" as const,
      account: "listener",
      key: state.route,
      generation: 2,
      playlists: [playlist],
      message: "",
    };
    expect(reducer(state, { ...event, key: "search:chill" })).toBe(state);
    expect(reducer(state, { ...event, generation: 1 })).toBe(state);
    expect(reducer(state, { ...event, account: "other" })).toBe(state);
    expect(reducer(emptyState, event)).toBe(emptyState);
    const loaded = reducer(state, event);
    expect(loaded.search_playlists).toEqual([playlist]);
    expect(loaded.search_playlists_loading).toBe(false);
    const next = reducer(loaded, {
      type: "navigate",
      key: "liked",
      generation: 3,
    });
    expect(next.search_playlists).toEqual([]);
    const back = reducer(next, {
      type: "navigate",
      key: event.key,
      generation: 4,
    });
    expect(back.search_playlists).toEqual([playlist]);
    expect(back.search_playlists_loading).toBe(true);
    expect(reducer(back, { type: "signed_out" }).search_playlists).toEqual([]);
  });
  it("clears a connecting notification once native audio becomes ready", () => {
    const connecting = reducer(emptyState, {
      type: "notice",
      message: "Connecting native audio…",
      error: false,
    });
    const ready = reducer(connecting, {
      type: "native",
      ready: true,
      status: "Ready · this computer",
    });
    expect(ready.toast).toBeUndefined();
    const failed = reducer(emptyState, {
      type: "notice",
      message: "Audio failed",
      error: true,
    });
    expect(
      reducer(failed, { type: "native", ready: true, status: "Ready" }).toast,
    ).toEqual(failed.toast);
  });
  it("keeps bundled lyrics configured after sign-out and sign-in", () => {
    const loaded = reducer(emptyState, {
      type: "bootstrap",
      snapshot: {
        ...emptyState,
        provider_builtin: true,
        provider_configured: true,
        provider_kind: "spicy",
      },
    });
    const signedOut = reducer(loaded, { type: "signed_out" });
    const signedIn = reducer(signedOut, {
      type: "session",
      profile: { id: "listener", display_name: "Listener", images: [] },
      can_follow: true,
    });
    expect(signedIn.provider_builtin).toBe(true);
    expect(signedIn.provider_configured).toBe(true);
    expect(signedIn.provider_kind).toBe("spicy");
    expect(signedOut.profile).toBeUndefined();
    expect(signedOut.playlists).toEqual([]);
  });
  it("keeps first-run setup hidden until saved sign-in restoration finishes", () => {
    expect(sessionScreen(emptyState)).toBe("restoring");
    const restoring = reducer(emptyState, {
      type: "bootstrap",
      snapshot: { ...emptyState, login_pending: true },
    });
    expect(sessionScreen(restoring)).toBe("restoring");
    const failed = reducer(restoring, { type: "login_finished" });
    expect(sessionScreen(failed)).toBe("setup");
    expect(sessionScreen(reducer(failed, { type: "login_pending" }))).toBe(
      "setup",
    );
    const restored = reducer(restoring, {
      type: "session",
      profile: {
        id: "listener",
        display_name: "Listener",
        images: [],
      },
      can_follow: false,
    });
    expect(sessionScreen(restored)).toBe("client");
    expect(sessionScreen(reducer(restored, { type: "signed_out" }))).toBe(
      "setup",
    );
  });
  it("rebases a batched same-song native takeover to actual audio position", () => {
    const song = { ...track, duration_ms: 180000 };
    const before = {
      ...emptyState,
      playback: {
        ...emptyState.playback,
        item: song,
        progress_ms: 77000,
        is_playing: true,
      },
    };
    const clock = new PlaybackClock();
    clock.observe(before.playback, 0, before.clock_revision, 0, 0);
    let state = reducer(before, {
      type: "playback",
      playback: { ...before.playback, is_playing: false },
      observed_at: 100,
      loading: true,
    } as any);
    state = reducer(state, {
      type: "native_position",
      uri: song.uri,
      position_ms: 1200,
      playing: true,
      seeked: false,
      observed_at: 100,
    });
    clock.observe(state.playback, 100, state.clock_revision, 100, 100);
    expect(clock.read(100)).toBe(1200);
  });
  it("opens a warmed Canvas immediately when its track is clicked", () => {
    const warm = reducer(emptyState, {
      type: "canvas_prefetch",
      uri: track.uri,
      url: "https://canvaz.scdn.co/test.mp4",
    });
    expect(reducer(warm, { type: "play_requested", track }).canvas_url).toBe(
      "https://canvaz.scdn.co/test.mp4",
    );
  });
  it("retains a warm banner during basic refresh and clears prefetch after sign-out", () => {
    const info = {
      key: "artist:test",
      title: "Artist",
      subtitle: "",
      images: [],
      albums: [],
      overview: { header_image: "https://i.scdn.co/banner" },
    } as any;
    let state = reducer(
      { ...emptyState, profile: { id: "account" } },
      { type: "info_prefetch", info },
    );
    state = reducer(state, { type: "navigate", key: info.key, generation: 1 });
    state = reducer(state, {
      type: "browse_info",
      generation: 1,
      info: { ...info, overview: undefined },
    });
    expect(state.browse_info?.overview?.header_image).toBe(
      "https://i.scdn.co/banner",
    );
    state = reducer(state, { type: "signed_out" });
    expect(
      reducer(state, {
        type: "page_prefetch",
        key: "liked",
        tracks: [track],
        more: false,
      }).pages,
    ).toEqual({});
  });
  it("shows whole lists only complete, never a first page", () => {
    let state = reducer(
      { ...emptyState, profile: { id: "account" } },
      { type: "page_prefetch", key: "playlist:a", tracks: [track], more: true },
    );
    state = reducer(state, {
      type: "navigate",
      key: "playlist:a",
      generation: 1,
    });
    expect(state.tracks).toEqual([]);
    expect(state.loading).toBe(true);
    // A first page arriving while it is open waits for the rest.
    state = reducer(state, {
      type: "page_prefetch",
      key: "playlist:a",
      tracks: [track],
      more: true,
    });
    expect(state.tracks).toEqual([]);
    state = reducer(state, {
      type: "tracks",
      key: "playlist:a",
      generation: 1,
      offset: 0,
      tracks: [track, { ...track, uri: "spotify:track:b" }],
      more: false,
    });
    expect(state.tracks).toHaveLength(2);
    // Opened again, the complete copy shows at once.
    state = reducer(state, { type: "navigate", key: "home", generation: 2 });
    state = reducer(state, {
      type: "navigate",
      key: "playlist:a",
      generation: 3,
    });
    expect(state.tracks).toHaveLength(2);
    // Other pages still show what they have.
    state = reducer(state, {
      type: "page_prefetch",
      key: "recent",
      tracks: [track],
      more: true,
    });
    state = reducer(state, { type: "navigate", key: "recent", generation: 4 });
    expect(state.tracks).toHaveLength(1);
  });
  it("alternates rapid play and pause before native acknowledgments arrive", () => {
    let previous: ToggleIntent | undefined;
    const choices = [];
    for (let n = 0; n < 6; n++) {
      previous = nextToggle(true, previous, n * 10);
      choices.push(previous.wanted);
    }
    expect(choices).toEqual([false, true, false, true, false, true]);
    expect(nextToggle(false, previous, 3000).wanted).toBe(true);
  });
  it("moves active focus to the next lead while the previous backing vocals continue", () => {
    const lines = [
      {
        start_ms: 1000,
        end_ms: 8000,
        words: [{ start_ms: 1000, end_ms: 2000, text: "Lead" }],
      },
      { start_ms: 1500, end_ms: 6000, words: [] },
      {
        start_ms: 3000,
        end_ms: 5000,
        words: [{ start_ms: 3000, end_ms: 5000, text: "Next" }],
      },
    ];
    expect(activeLines(lines, 3500)).toEqual([1, 2]);
  });
  it("preserves imported lyrics when an earlier provider request fails", () => {
    const lyrics = {
      lines: [],
      source: "Local TTML",
      synced: true,
      instrumental: false,
    };
    const state = {
      ...emptyState,
      playback: { ...emptyState.playback, item: track },
      lyrics,
    };
    expect(
      reducer(state, {
        type: "lyric_error",
        uri: track.uri,
        message: "Provider unavailable",
      }).lyrics,
    ).toBe(lyrics);
  });
  it("marks pagination busy before the response and rejects stale loading events", () => {
    const state = { ...emptyState, generation: 3, tracks: [track] };
    expect(
      reducer(state, { type: "page_loading", generation: 3 }).loading,
    ).toBe(true);
    expect(reducer(state, { type: "page_loading", generation: 2 })).toBe(state);
  });
  it("accepts native track changes after next while waiting on an earlier load", () => {
    const state = reducer(emptyState, { type: "play_requested", track });
    const next = { ...track, uri: "spotify:track:next" };
    const changed = reducer(state, {
      type: "playback",
      playback: { ...emptyState.playback, item: next },
      observed_at: Date.now(),
      changed: true,
      loading: true,
    });
    expect(changed.playback.item?.uri).toBe(next.uri);
    expect(changed.pending?.uri).toBe(next.uri);
  });
  it("discards search results from an older query", () => {
    const current = reducer(emptyState, {
      type: "navigate",
      key: "search:second",
      generation: 2,
    });
    expect(
      reducer(current, {
        type: "tracks",
        key: "search:first",
        generation: 1,
        offset: 0,
        tracks: [track],
        more: false,
      }),
    ).toBe(current);
  });
  it("updates pause immediately from the native event", () => {
    const current = {
      ...emptyState,
      playback: { ...emptyState.playback, item: track, is_playing: true },
    };
    const paused = reducer(current, {
      type: "native_position",
      uri: track.uri,
      position_ms: 1200,
      playing: false,
      observed_at: 100,
    });
    expect(paused.playback.is_playing).toBe(false);
    expect(position(paused.playback, 100, 1000)).toBe(1200);
  });
  it("stops the loading spinner when a requested song ends up paused", () => {
    const loading = reducer(emptyState, { type: "play_requested", track });
    expect(loading.pending).toBeDefined();
    const paused = reducer(loading, {
      type: "native_position",
      uri: track.uri,
      position_ms: 0,
      playing: false,
      observed_at: 100,
    });
    expect(paused.pending).toBeUndefined();
    expect(paused.playback.is_playing).toBe(false);
  });
  it("stops the loading spinner when a start fails, but not for other notices", () => {
    const loading = reducer(emptyState, { type: "play_requested", track });
    const info = reducer(loading, {
      type: "notice",
      message: "Added to queue",
      error: false,
    });
    expect(info.pending).toBeDefined();
    const failed = reducer(loading, {
      type: "notice",
      message: "Spotify didn't start that. Press Play to try again.",
      error: true,
      stalled: true,
    });
    expect(failed.pending).toBeUndefined();
    expect(failed.toast?.message).toContain("didn't start");
  });
  it("ignores lyrics for a previous track", () => {
    const current = {
      ...emptyState,
      playback: { ...emptyState.playback, item: track },
    };
    expect(
      reducer(current, { type: "lyric_error", uri: "old", message: "missing" }),
    ).toBe(current);
  });
  it("does not replace a newly clicked song with stale Spotify state", () => {
    const current = reducer(emptyState, { type: "play_requested", track });
    expect(
      reducer(current, {
        type: "playback",
        playback: emptyState.playback,
        observed_at: Date.now(),
        changed: true,
      }),
    ).toBe(current);
  });
});
describe("lyric timing", () => {
  it("keeps overlapping lead and background lines active", () => {
    expect(
      activeLines(
        [
          { start_ms: 0, end_ms: 2000 },
          { start_ms: 500, end_ms: 1500 },
        ],
        1000,
      ),
    ).toEqual([0, 1]);
  });
  it("clamps word progress and handles malformed zero duration", () => {
    expect(progress(100, 200, 50)).toBe(0);
    expect(progress(100, 200, 300)).toBe(1);
    expect(progress(100, 100, 100)).toBe(0);
  });
});
describe("personalized Home ownership", () => {
  it("keeps generated playlist metadata through navigation and incomplete responses", () => {
    const item = {
      uri: "spotify:playlist:1123456789abcdefghijkl",
      route: "playlist:1123456789abcdefghijkl",
      name: "Daily Mix 1",
      subtitle: "Your favorite artists",
      images: [{ url: "https://artwork.example.test/mix" }],
      kind: "playlist",
      playable: true,
      format: "daily-mix",
    };
    let state = reducer(
      { ...emptyState, profile: { id: "account-a" } },
      {
        type: "home",
        page: {
          account: "account-a",
          greeting: "Hello",
          sections: [
            { uri: "mixes", title: "Mixes", subtitle: "", items: [item] },
          ],
        },
      },
    );
    state = reducer(state, {
      type: "navigate",
      key: item.route,
      generation: 1,
    });
    expect(state.browse_info?.title).toBe(item.name);
    state = reducer(state, {
      type: "browse_info",
      generation: 1,
      info: {
        key: item.route,
        title: "",
        subtitle: "",
        images: [],
        albums: [],
      },
    });
    expect(state.browse_info?.title).toBe(item.name);
    expect(state.browse_info?.images).toEqual(item.images);
    const native = {
      ...state.browse_info!,
      playlist: { owner: "spotify", total_tracks: 50, format: "daily-mix" },
    };
    state = reducer(state, {
      type: "browse_info",
      generation: 1,
      info: native,
    });
    state = reducer(state, { type: "navigate", key: "home", generation: 2 });
    state = reducer(state, {
      type: "navigate",
      key: item.route,
      generation: 3,
    });
    expect(state.browse_info?.playlist?.total_tracks).toBe(50);
  });
  it("rejects another account's feed and clears shelves when account changes", () => {
    const current = { ...emptyState, profile: { id: "account-a" } };
    const page = { account: "account-a", greeting: "Hello", sections: [] };
    const loaded = reducer(current, { type: "home", page });
    expect(loaded.home).toBe(page);
    expect(
      reducer(current, {
        type: "home",
        page: { ...page, account: "account-b" },
      }),
    ).toBe(current);
    const changed = reducer(loaded, {
      type: "session",
      profile: { id: "account-b" },
      can_follow: false,
    });
    expect(changed.home).toBeUndefined();
    expect(reducer(changed, { type: "home", page })).toBe(changed);
    expect(
      reducer(changed, {
        type: "home_error",
        account: "account-a",
        message: "old",
      }),
    ).toBe(changed);
  });
  it("drops mismatched shelves during session restore", () => {
    const restored = reducer(emptyState, {
      type: "bootstrap",
      snapshot: {
        ...emptyState,
        profile: { id: "account-b" },
        home: { account: "account-a", greeting: "Hello", sections: [] },
      },
    });
    expect(restored.home).toBeUndefined();
  });
});
describe("sign-in recovery", () => {
  it("keeps a restore failure visible until retry or successful sign-in", () => {
    const issue = {
      message: "Cannot reach Spotify sign-in service",
      retry_saved: true,
    };
    const failed = reducer(
      {
        ...emptyState,
        booted: true,
        restoring_session: true,
        login_pending: true,
      },
      { type: "login_failed", issue },
    );
    expect(failed).toMatchObject({
      login_pending: false,
      restoring_session: false,
      login_issue: issue,
    });
    expect(sessionScreen(failed)).toBe("setup");
    expect(reducer(failed, { type: "toast_clear" }).login_issue).toEqual(issue);
    const retried = reducer(failed, { type: "login_pending", restoring: true });
    expect(retried.login_issue).toBeUndefined();
    expect(sessionScreen(retried)).toBe("restoring");
    const signedIn = reducer(failed, {
      type: "session",
      profile: { id: "account" },
      can_follow: false,
    });
    expect(signedIn.login_issue).toBeUndefined();
    expect(sessionScreen(signedIn)).toBe("client");
  });
});

describe("starting DJ", () => {
  const signedIn = {
    ...emptyState,
    profile: { id: "account" },
    native_ready: true,
  };
  it("shows DJ at once so the first queued song cannot flash up", () => {
    const requested = reducer(signedIn, { type: "dj_requested" });
    expect(requested.narrating).toBe(true);
    // The old song stopping reports no narration; DJ stays up.
    const quiet = reducer(requested, { type: "narration", active: false });
    expect(quiet.narrating).toBe(true);
    const speaking = reducer(quiet, { type: "narration", active: true });
    expect(speaking).toMatchObject({ narrating: true, dj_pending: false });
    // Once confirmed, narration ending returns to the song.
    expect(
      reducer(speaking, { type: "narration", active: false }).narrating,
    ).toBe(false);
    expect(reducer(speaking, { type: "dj_pending_expired" })).toBe(speaking);
  });
  it("falls back to the song when DJ never starts talking", () => {
    const requested = reducer(signedIn, { type: "dj_requested" });
    expect(reducer(requested, { type: "dj_pending_expired" }).narrating).toBe(
      false,
    );
    expect(reducer(emptyState, { type: "dj_requested" }).narrating).toBe(false);
  });
});

describe("library edits on the open page", () => {
  const other: Track = { ...track, id: "other", uri: "spotify:track:other" };
  it("drops an unliked song from the open Liked Songs list at once", () => {
    const liked = { ...emptyState, route: "liked", tracks: [track, other] };
    const next = reducer(liked, {
      type: "saved",
      uri: track.uri,
      saved: false,
    });
    expect(next.tracks).toEqual([other]);
    expect(next.saved[track.uri]).toBe(false);
    expect(next.pages.liked.tracks).toEqual([other]);
    const elsewhere = { ...liked, route: "playlist:p" };
    expect(
      reducer(elsewhere, { type: "saved", uri: track.uri, saved: false })
        .tracks,
    ).toHaveLength(2);
  });
  it("moves a dragged row and ignores other pages", () => {
    const third: Track = { ...track, id: "third", uri: "spotify:track:third" };
    const open = {
      ...emptyState,
      route: "playlist:p",
      tracks: [track, other, third],
    };
    const next = reducer(open, {
      type: "track_moved",
      key: "playlist:p",
      from: 0,
      to: 2,
    });
    expect(next.tracks).toEqual([other, third, track]);
    expect(
      reducer(open, { type: "track_moved", key: "playlist:q", from: 0, to: 2 })
        .tracks,
    ).toEqual(open.tracks);
  });
  it("removes only the chosen row of the open playlist", () => {
    const open = {
      ...emptyState,
      route: "playlist:p",
      tracks: [track, other, track],
    };
    const next = reducer(open, {
      type: "track_removed",
      key: "playlist:p",
      index: 2,
    });
    expect(next.tracks).toEqual([track, other]);
    expect(
      reducer(open, { type: "track_removed", key: "playlist:q", index: 0 })
        .tracks,
    ).toHaveLength(3);
  });
});

describe("profile refresh", () => {
  it("updates the signed-in profile only", () => {
    const signedIn = {
      ...emptyState,
      profile: { id: "me", display_name: "Old", images: [] },
    } as AppState;
    const refreshed = reducer(signedIn, {
      type: "profile",
      profile: { id: "me", display_name: "New", images: [] },
    });
    expect(refreshed.profile?.display_name).toBe("New");
    const other = reducer(signedIn, {
      type: "profile",
      profile: { id: "someone", display_name: "Else", images: [] },
    });
    expect(other).toBe(signedIn);
  });
});

describe("queue across song changes", () => {
  const song = (uri: string): Track => ({ ...track, uri, name: uri });
  const [a, b, c, d] = ["a", "b", "c", "d"].map(song);
  it("moves up on skip and back down on previous", () => {
    const skipped = advanceDetails({ artists: [], queue: [b, c, d] }, a, b);
    expect(skipped.queue.map((item) => item.uri)).toEqual(["c", "d"]);
    const back = advanceDetails(skipped, b, a);
    expect(back.queue.map((item) => item.uri)).toEqual(["b", "c", "d"]);
    expect(back.played).toEqual([]);
  });
  it("counts down queued songs and returns Previous after them", () => {
    const skipped = advanceDetails(
      { artists: [], queue: [b, c, d], queued: 2 },
      a,
      b,
    );
    expect(skipped.queued).toBe(1);
    // The old song goes back to the playlist, behind the queued song.
    const back = advanceDetails(skipped, b, a);
    expect(back.queue.map((item) => item.uri)).toEqual(["c", "b", "d"]);
    expect(back.queued).toBe(1);
    const past = advanceDetails(
      { artists: [], queue: [b, c, d], queued: 1 },
      a,
      c,
    );
    expect(past.queued).toBe(0);
  });
  it("skips over songs picked further down the queue", () => {
    const picked = advanceDetails({ artists: [], queue: [b, c, d] }, a, c);
    expect(picked.queue.map((item) => item.uri)).toEqual(["d"]);
  });
  it("waits for Spotify when the new song was not queued", () => {
    const other = advanceDetails({ artists: [], queue: [b, c] }, a, d);
    expect(other.queue).toEqual([]);
    expect(other.played?.map((item) => item.uri)).toEqual(["a"]);
  });
  it("holds a queue Spotify advanced before the song changed", () => {
    const early = advanceDetails({ artists: [], queue: [c, d] }, a, b, true);
    expect(early.queue.map((item) => item.uri)).toEqual(["c", "d"]);
    const back = advanceDetails(
      { artists: [], queue: [a, c], played: [b] },
      a,
      b,
    );
    expect(back.queue.map((item) => item.uri)).toEqual(["a", "c"]);
  });
  it("keeps the queue while the next one arrives", () => {
    const state = reducer(
      {
        ...emptyState,
        playback: { ...emptyState.playback, item: a },
        details: { artists: [], queue: [b, c] },
      },
      {
        type: "playback",
        playback: { ...emptyState.playback, item: b },
        observed_at: 0,
      } as never,
    );
    expect(state.details.queue.map((item) => item.uri)).toEqual(["c"]);
  });
});
