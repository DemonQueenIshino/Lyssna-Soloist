import type {
  AppEvent,
  ArtistAbout,
  BrowseInfo,
  Device,
  Jam,
  Snapshot,
  PlayingDetails,
  Track,
  Playlist,
  LibraryFolder,
  Artist,
  Album,
  TrackFacts,
  Word,
  Profile,
  SearchResults,
} from "./types";
/** A page as last loaded, shown instantly when revisited or prefetched. */
export interface CachedPage {
  search_playlists?: Playlist[];
  search_artists?: Artist[];
  search_albums?: Album[];
  search_top_results?: string[];
  search_users?: Profile[];
  tracks?: Track[];
  more?: boolean;
  info?: BrowseInfo;
}
const PAGE_LIMIT = 40;
/** The longest list a cached page keeps. */
const KEPT_SONGS = 1000;
function rememberCanvas(
  previous: AppState["canvases"],
  uri: string,
  url: string | undefined,
) {
  const copy = { ...previous };
  delete copy[uri];
  return { ...Object.fromEntries(Object.entries(copy).slice(-63)), [uri]: url };
}
function mergeInfo(
  previous: BrowseInfo | undefined,
  incoming: BrowseInfo,
): BrowseInfo {
  return previous?.key === incoming.key
    ? {
        ...incoming,
        title: incoming.title || previous.title,
        subtitle: incoming.title ? incoming.subtitle : previous.subtitle,
        images: incoming.images.length ? incoming.images : previous.images,
        playlist: incoming.playlist ?? previous.playlist,
        artist: incoming.artist ?? previous.artist,
        album: incoming.album ?? previous.album,
        overview: incoming.overview ?? previous.overview,
        album_details: incoming.album_details ?? previous.album_details,
      }
    : incoming;
}
/** Lists that show every song at once, like Spotify: a playlist, Liked
 * Songs, an artist's liked songs or an album. A part of one is never shown,
 * so the list doesn't grow or reshuffle as the rest arrives. */
export function loadsWhole(key: string) {
  return (
    key === "liked" ||
    ["playlist:", "liked-artist:", "album:"].some((prefix) =>
      key.startsWith(prefix),
    )
  );
}

export function rememberPage(
  pages: Record<string, CachedPage>,
  key: string,
  page: CachedPage,
): Record<string, CachedPage> {
  const { [key]: previous, ...rest } = pages;
  const keys = Object.keys(rest);
  for (const old of keys.slice(0, Math.max(0, keys.length - PAGE_LIMIT + 1)))
    delete rest[old];
  return {
    ...rest,
    [key]: {
      ...previous,
      ...page,
      info: page.info ? mergeInfo(previous?.info, page.info) : previous?.info,
    },
  };
}
export type ToggleIntent = { wanted: boolean; at: number };
export function nextToggle(
  current: boolean,
  previous: ToggleIntent | undefined,
  now: number,
): ToggleIntent {
  return {
    wanted: !(previous && now - previous.at < 2000 ? previous.wanted : current),
    at: now,
  };
}
export interface AppState extends Snapshot {
  restoring_session: boolean;
  canvases: Record<string, string | undefined>;
  /** About details by artist ID, upcoming artists included. */
  artist_about: Record<string, ArtistAbout>;
  facts: Record<string, TrackFacts>;
  booted: boolean;
  devices: Device[];
  saved: Record<string, boolean>;
  toast?: { message: string; error: boolean; serial: number };
  pending?: { uri: string; at: number };
  pageError: string;
  clock_revision: number;
  pages: Record<string, CachedPage>;
  /** DJ was just started and its narration has not been confirmed yet. The
   * DJ view shows meanwhile, so the first queued song cannot flash up. */
  dj_pending?: boolean;
  /** The playing context is a DJ session, so DJ controls apply. */
  dj_context?: boolean;
  /** Counts Spotify's Jam changes, so the Jam panel asks for it again. */
  jam_revision?: number;
  /** The Jam as Spotify's last change report gave it, members included. */
  jam_report?: Jam | null;
  /** Spotify folders, so empty ones still show in the sidebar. */
  library_folders?: LibraryFolder[];
  /** A creator profile sheet is open over the app, which dims behind it. */
  creator_profile_open?: boolean;
}
export const emptyState: AppState = {
  narrating: false,
  home: undefined,
  home_loading: false,
  home_error: "",
  canvases: {},
  artist_about: {},
  facts: {},
  booted: false,
  restoring_session: true,
  profile: undefined,
  playlists: [],
  playback: {
    is_playing: false,
    shuffle_state: false,
    repeat_state: "off",
    device: { name: "", is_active: false },
  },
  settings: {
    client_id: "",
    last_view: "home",
    show_lyrics: true,
    compact_library: false,
    show_local_files: true,
    local_sources: [],
    lyric_size: 36,
    lyric_offset_ms: 0,
    romanized: false,
    reduce_motion: false,
    animation_fps_cap_enabled: false,
    animation_fps_cap: 60,
    cover_animation: true,
    liquid_glass: false,
    canvas_lyrics: true,
    npv_ambience: true,
    glass_seek: true,
    space_gravity: false,
    lyrics_highlighter: false,
    library_sort: "recents",
    library_view: "list",
    lyrics_highlighter_color: "#d4b80f",
    gravity_links: true,
    gravity_link_width: 100,
    lyrics_translations: false,
    experimental_word_sync: false,
    lyrics_hide_artwork: false,
    lyrics_artwork_right: false,
    lyrics_compact: false,
    lyrics_toolbar_bottom: false,
    lyrics_right_align: false,
    lyrics_flip_duet: false,
    lyrics_wide_duet: true,
    lyrics_credits_follow: false,
    lyrics_simple: false,
    lyrics_minimal: false,
    lyrics_keep_overlaps: true,
    lyrics_word_filter: "off",
    lyrics_line_hover: false,
    lyrics_skeleton: true,
    lyrics_no_lyrics_notice: true,
    lyrics_cache_button: true,
    lyrics_cache_action: "song",
    lyrics_early_scroll: true,
    lyrics_follow_button: true,
    lyrics_smooth_scroll: true,
    lyrics_early_scroll_ms: 200,
    lyrics_seek_compensation: true,
    lyrics_year_position: "left",
    lyrics_hide_npv: false,
    lyrics_hide_npv_empty: false,
    now_playing_open: true,
    song_sorts: {},
    lyrics_background: "animated",
    lyrics_background_blur: 0,
    lyrics_pinned_footer: "off",
    fullscreen_always_show: "none",
    fullscreen_volume: "off",
    fullscreen_animate_close: false,
    lyrics_source_order: [
      "spicy",
      "apple",
      "musixmatch",
      "spotify",
      "lrclib",
      "netease",
    ],
    lyrics_sources_disabled: ["lrclib", "netease"],
    lyrics_prefer_apple: true,
    lyrics_musixmatch_lines_only: false,
    settings_remember_place: true,
    ui_font: "",
    lyric_font: "",
    audio_output: null,
    auto_download_updates: true,
    update_channel: "stable",
    folders: [],
  },
  native_ready: false,
  local_device_name: "Lyssna",
  native_status: "Connecting…",
  provider_configured: false,
  login_pending: false,
  can_follow: false,
  lyrics: undefined,
  lyric_error: "",
  details: { artists: [], queue: [] },
  route: "home",
  generation: 0,
  tracks: [],
  search_playlists: [],
  search_artists: [],
  search_albums: [],
  search_top_results: [],
  search_users: [],
  search_playlists_loading: false,
  search_playlists_error: "",
  loading: false,
  more: false,
  update_status: "Checking for updates…",
  notice: "",
  observed_at: Date.now(),
  devices: [],
  saved: {},
  pageError: "",
  clock_revision: 0,
  pages: {},
};
export function sessionScreen(
  state: Pick<AppState, "booted" | "profile" | "restoring_session">,
): "restoring" | "setup" | "client" {
  return !state.booted || (!state.profile && state.restoring_session)
    ? "restoring"
    : state.profile
      ? "client"
      : "setup";
}
export type Action =
  | AppEvent
  | { type: "bootstrap"; snapshot: Snapshot }
  | { type: "navigate"; key: string; generation: number }
  | { type: "search_more"; key: string; results: SearchResults }
  | { type: "play_requested"; track: Track }
  | { type: "settings"; settings: AppState["settings"] }
  | { type: "login_pending"; restoring?: boolean }
  | { type: "page_loading"; generation: number }
  | { type: "toast_clear" }
  | { type: "dj_requested" }
  /** A song left the open playlist; drop that row before Spotify confirms. */
  | { type: "track_removed"; key: string; index: number }
  /** A song was dragged within the open playlist. */
  | { type: "track_moved"; key: string; from: number; to: number }
  | { type: "dj_pending_expired" };
/** The queue right after the song changes, before Spotify sends it: a
 * skip moves up to the new song and Previous puts the old one back on top,
 * so the panel never empties and its rows can slide into place. Spotify can
 * publish the advanced queue just before the song itself changes; `keep`
 * then holds that queue when the new song is no longer in it. */
export function advanceDetails(
  details: PlayingDetails,
  from: Track | null | undefined,
  to: Track | null | undefined,
  keep = false,
): PlayingDetails {
  const played = details.played ?? [];
  const queued = details.queued ?? 0;
  if (!to) return { artists: [], queue: [], played: [], queued: 0 };
  // Previous returns the old song to the playlist, after the songs the
  // listener queued, as in Spotify.
  if (played.at(-1)?.uri === to.uri)
    return {
      artists: [],
      queue:
        from && details.queue[queued]?.uri !== from.uri
          ? [
              ...details.queue.slice(0, queued),
              from,
              ...details.queue.slice(queued),
            ]
          : details.queue,
      played: played.slice(0, -1),
      queued,
    };
  const at = details.queue.findIndex((track) => track.uri === to.uri);
  return {
    artists: [],
    queue: at >= 0 ? details.queue.slice(at + 1) : keep ? details.queue : [],
    played: from ? [...played, from].slice(-20) : played,
    queued: at >= 0 ? Math.max(0, queued - at - 1) : keep ? queued : 0,
  };
}
export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "narration":
      // While DJ starts, a "not speaking" report from the old track must not
      // drop the DJ view before the first narration begins.
      if (!action.active && state.dj_pending) return state;
      return {
        ...state,
        dj_pending: false,
        narrating: !!state.profile && state.native_ready && action.active,
      };
    // Played by App; nothing to keep.
    case "play_uri":
    case "link":
    case "local_files":
      return state;
    case "creator_profile":
      return { ...state, creator_profile_open: action.open };
    case "dj_context":
      return { ...state, dj_context: action.active };
    case "jam_changed":
      return {
        ...state,
        jam_revision: (state.jam_revision ?? 0) + 1,
        jam_report: action.jam ?? null,
      };
    case "dj_requested":
      return state.profile
        ? { ...state, narrating: true, dj_pending: true }
        : state;
    case "dj_pending_expired":
      // DJ never started talking; show whatever is playing.
      return state.dj_pending
        ? { ...state, dj_pending: false, narrating: false }
        : state;
    case "track_facts":
      return state.profile
        ? { ...state, facts: { ...state.facts, ...action.facts } }
        : state;
    case "artist_about": {
      const kept = { ...state.artist_about };
      delete kept[action.id];
      return {
        ...state,
        artist_about: {
          ...Object.fromEntries(Object.entries(kept).slice(-127)),
          [action.id]: action.about,
        },
      };
    }
    case "canvas_prefetch":
      return {
        ...state,
        canvases: rememberCanvas(state.canvases, action.uri, action.url),
        canvas_url:
          state.playback.item?.uri === action.uri
            ? action.url
            : state.canvas_url,
      };
    case "page_prefetch": {
      if (!state.profile) return state;
      const pages = rememberPage(state.pages, action.key, {
        tracks: action.tracks,
        more: action.more,
      });
      // Arrived while the page is open and still empty: show it now.
      return action.key === state.route &&
        !state.tracks.length &&
        !(action.more && loadsWhole(action.key))
        ? { ...state, pages, tracks: action.tracks, more: action.more }
        : { ...state, pages };
    }
    case "info_prefetch":
      if (!state.profile) return state;
      return {
        ...state,
        pages: rememberPage(state.pages, action.info.key, {
          info: action.info,
        }),
        browse_info:
          action.info.key === state.route
            ? mergeInfo(state.browse_info, action.info)
            : state.browse_info,
      };
    case "canvas":
      return {
        ...state,
        canvases: rememberCanvas(state.canvases, action.uri, action.url),
        canvas_url:
          state.playback.item?.uri === action.uri
            ? action.url
            : state.canvas_url,
      };
    case "bootstrap":
      return {
        ...state,
        ...action.snapshot,
        restoring_session:
          !action.snapshot.profile && action.snapshot.login_pending,
        home:
          action.snapshot.home?.account === action.snapshot.profile?.id
            ? action.snapshot.home
            : undefined,
        booted: true,
      };
    case "session":
      return {
        ...state,
        login_issue: undefined,
        restoring_session: false,
        narrating: state.profile?.id === action.profile.id && state.narrating,
        home:
          state.home?.account === action.profile.id ? state.home : undefined,
        home_loading: false,
        home_error: "",
        profile: action.profile,
        can_follow: action.can_follow,
        login_pending: false,
      };
    case "profile":
      // Spotify's current name and picture, after a restore unlocked with
      // the saved ones.
      return state.profile?.id === action.profile.id
        ? { ...state, profile: action.profile }
        : state;
    case "signed_out":
      return {
        ...emptyState,
        restoring_session: false,
        booted: true,
        settings: state.settings,
        provider_builtin: state.provider_builtin,
        provider_configured: state.provider_configured,
        provider_kind: state.provider_kind,
        native_status: "Waiting for sign-in",
      };
    case "home_loading":
      return action.account === state.profile?.id
        ? { ...state, home_loading: true, home_error: "" }
        : state;
    case "home":
      return action.page.account === state.profile?.id
        ? { ...state, home: action.page, home_loading: false, home_error: "" }
        : state;
    case "home_error":
      return action.account === state.profile?.id
        ? { ...state, home_loading: false, home_error: action.message }
        : state;
    case "login_pending":
      return {
        ...state,
        login_pending: true,
        login_issue: undefined,
        restoring_session: !!action.restoring,
      };
    case "login_failed":
      return {
        ...state,
        login_pending: false,
        restoring_session: false,
        login_issue: action.issue,
      };
    case "login_finished":
      return { ...state, login_pending: false, restoring_session: false };
    case "native":
      return {
        ...state,
        narrating: action.ready && state.narrating,
        native_ready: action.ready,
        native_status: action.status,
        toast:
          action.ready &&
          !state.toast?.error &&
          state.toast?.message === "Connecting native audio…"
            ? undefined
            : state.toast,
      };
    case "library_folders":
      return { ...state, library_folders: action.folders };
    case "library":
      return { ...state, playlists: action.playlists };
    case "search_playlists":
      if (
        action.account !== state.profile?.id ||
        action.generation !== state.generation ||
        action.key !== state.route
      )
        return state;
      return {
        ...state,
        search_playlists: action.playlists,
        search_playlists_loading: false,
        search_playlists_error: action.message,
        pages: rememberPage(state.pages, action.key, {
          search_playlists: action.playlists,
        }),
      };
    case "search_results": {
      if (
        action.account !== state.profile?.id ||
        action.generation !== state.generation ||
        action.key !== state.route
      )
        return state;
      const page = {
        tracks: action.results.tracks,
        search_playlists: action.results.playlists,
        search_artists: action.results.artists,
        search_albums: action.results.albums,
        search_top_results: action.results.top_results,
        search_users: action.results.users ?? [],
        more: false,
      };
      return {
        ...state,
        ...page,
        loading: false,
        pageError: "",
        search_playlists_loading: false,
        search_playlists_error: action.message,
        pages: rememberPage(state.pages, action.key, page),
      };
    }
    case "search_more": {
      // A further page of one category; stale when the search changed.
      if (action.key !== state.route) return state;
      const add = <T>(current: T[], next: T[], key: (item: T) => string) => {
        const seen = new Set(current.map(key));
        return [...current, ...next.filter((item) => !seen.has(key(item)))];
      };
      const page = {
        tracks: add(state.tracks, action.results.tracks, (item) => item.uri),
        search_artists: add(
          state.search_artists,
          action.results.artists,
          (item) => item.uri || item.id || "",
        ),
        search_albums: add(
          state.search_albums,
          action.results.albums,
          (item) => item.uri || item.id || "",
        ),
        search_playlists: add(
          state.search_playlists,
          action.results.playlists,
          (item) => item.id,
        ),
        search_users: add(
          state.search_users ?? [],
          action.results.users ?? [],
          (item) => item.id,
        ),
      };
      return {
        ...state,
        ...page,
        pages: rememberPage(state.pages, action.key, page),
      };
    }
    case "page_loading":
      return action.generation === state.generation
        ? { ...state, loading: true }
        : state;
    case "navigate": {
      // Show the cached copy at once; fresh data replaces it when it lands.
      const cached = state.pages[action.key];
      const cachedTracks =
        cached?.more && loadsWhole(action.key) ? [] : cached?.tracks;
      const item =
        state.home?.account === state.profile?.id
          ? state.home?.sections
              .flatMap((section) => section.items)
              .find((item) => item.route === action.key)
          : undefined;
      const homeInfo: BrowseInfo | undefined = item
        ? {
            key: action.key,
            title: item.name,
            subtitle: item.subtitle,
            images: item.images,
            albums: [],
            playlist:
              item.kind === "playlist"
                ? { owner: "", format: item.format }
                : undefined,
          }
        : undefined;
      const info = cached?.info ? mergeInfo(homeInfo, cached.info) : homeInfo;
      return {
        ...state,
        route: action.key,
        generation: action.generation,
        tracks: cachedTracks ?? [],
        search_playlists: cached?.search_playlists ?? [],
        search_artists: cached?.search_artists ?? [],
        search_albums: cached?.search_albums ?? [],
        search_top_results: cached?.search_top_results ?? [],
        search_users: cached?.search_users ?? [],
        search_playlists_loading: action.key.startsWith("search:"),
        search_playlists_error: "",
        browse_info: info,
        pages: info
          ? rememberPage(state.pages, action.key, { info })
          : state.pages,
        loading: action.key !== "home",
        more: false,
        pageError: "",
      };
    }
    case "tracks":
      if (action.generation !== state.generation || action.key !== state.route)
        return state;
      return {
        ...state,
        tracks:
          action.offset === 0
            ? action.tracks
            : action.offset === state.tracks.length
              ? [...state.tracks, ...action.tracks]
              : state.tracks,
        loading: false,
        more: action.more,
        pageError: "",
        // Long lists aren't kept in memory once left; they reload quickly.
        pages:
          action.offset === 0 && action.tracks.length <= KEPT_SONGS
            ? rememberPage(state.pages, action.key, {
                tracks: action.tracks,
                more: action.more,
              })
            : state.pages,
      };
    case "tracks_failed":
      return action.generation === state.generation
        ? { ...state, loading: false, pageError: action.message }
        : state;
    case "browse_info":
      return action.generation === state.generation &&
        action.info.key === state.route
        ? {
            ...state,
            loading:
              state.route.startsWith("artist:") && action.info.overview
                ? false
                : state.loading,
            browse_info: mergeInfo(state.browse_info, action.info),
            pages: rememberPage(state.pages, action.info.key, {
              info: action.info,
            }),
          }
        : state;
    case "play_requested":
      return {
        ...state,
        canvas_url: state.canvases[action.track.uri],
        pending: { uri: action.track.uri, at: Date.now() },
        playback: {
          ...state.playback,
          item: action.track,
          progress_ms: 0,
          is_playing: false,
        },
        observed_at: Date.now(),
        lyrics: undefined,
        lyric_error: "",
        details:
          state.playback.item?.uri === action.track.uri
            ? state.details
            : advanceDetails(state.details, state.playback.item, action.track),
      };
    case "playback": {
      if (
        state.pending &&
        !action.loading &&
        action.playback.item?.uri !== state.pending.uri &&
        Date.now() - state.pending.at < 8000
      )
        return state;
      const changed = state.playback.item?.uri !== action.playback.item?.uri;
      return {
        ...state,
        clock_revision: state.clock_revision + (action.loading ? 1 : 0),
        playback: action.playback,
        observed_at: action.observed_at,
        pending:
          action.loading && action.playback.item
            ? { uri: action.playback.item.uri, at: Date.now() }
            : action.playback.is_playing
              ? undefined
              : state.pending,
        lyrics: changed ? undefined : state.lyrics,
        canvas_url: changed
          ? state.canvases[action.playback.item?.uri ?? ""]
          : state.canvas_url,
        lyric_error: changed ? "" : state.lyric_error,
        details: changed
          ? advanceDetails(
              state.details,
              state.playback.item,
              action.playback.item,
              true,
            )
          : state.details,
      };
    }
    case "native_position":
      if (action.uri !== state.playback.item?.uri) return state;
      return {
        ...state,
        playback: {
          ...state.playback,
          progress_ms: action.position_ms,
          is_playing: action.playing ?? state.playback.is_playing,
        },
        observed_at: action.observed_at,
        // Any play or pause report for this song settles the load, so a
        // start that failed into a pause no longer spins forever.
        pending:
          action.playing !== undefined || state.playback.is_playing
            ? undefined
            : state.pending,
        clock_revision: state.clock_revision + (action.seeked ? 1 : 0),
      };
    case "playing_details":
      return action.uri === state.playback.item?.uri
        ? {
            ...state,
            details: { ...action.details, played: state.details.played },
          }
        : state;
    case "lyrics":
      return action.uri === state.playback.item?.uri
        ? { ...state, lyrics: action.lyrics, lyric_error: "" }
        : state;
    case "lyric_error":
      return action.uri === state.playback.item?.uri
        ? { ...state, lyric_error: action.message }
        : state;
    case "devices":
      return { ...state, devices: action.devices };
    case "saved": {
      const saved = { ...state.saved, [action.uri]: action.saved };
      // An unliked song leaves the open Liked Songs list at once.
      if (
        action.saved ||
        (state.route !== "liked" && !state.route.startsWith("liked-artist:"))
      )
        return { ...state, saved };
      const tracks = state.tracks.filter((track) => track.uri !== action.uri);
      return {
        ...state,
        saved,
        tracks,
        pages: rememberPage(state.pages, state.route, {
          tracks,
          more: state.more,
        }),
      };
    }
    case "track_moved": {
      const { from, to } = action;
      if (
        action.key !== state.route ||
        !state.tracks[from] ||
        to < 0 ||
        to >= state.tracks.length
      )
        return state;
      const tracks = [...state.tracks];
      tracks.splice(to, 0, ...tracks.splice(from, 1));
      return {
        ...state,
        tracks,
        pages: rememberPage(state.pages, state.route, {
          tracks,
          more: state.more,
        }),
      };
    }
    case "track_removed": {
      if (action.key !== state.route || !state.tracks[action.index])
        return state;
      const tracks = state.tracks.filter((_, index) => index !== action.index);
      return {
        ...state,
        tracks,
        pages: rememberPage(state.pages, state.route, {
          tracks,
          more: state.more,
        }),
      };
    }
    case "saved_many":
      return { ...state, saved: { ...state.saved, ...action.items } };
    case "settings":
      return { ...state, settings: action.settings };
    case "provider_configured":
      return {
        ...state,
        provider_configured: true,
        provider_kind: action.kind,
        lyric_error: "",
        toast: {
          message: "Lyrics source saved",
          error: false,
          serial: Date.now(),
        },
      };
    case "notice":
      return {
        ...state,
        // A failed start reports no play or pause; settle the spinner here.
        pending: action.stalled ? undefined : state.pending,
        toast: {
          message: action.message,
          error: action.error,
          serial: Date.now(),
        },
      };
    case "toast_clear":
      return { ...state, toast: undefined };
    case "media_control":
      // App handles these through the playback commands.
      return state;
    case "window_moved":
    case "window_activity":
      // Held outside React state so animations update without re-rendering.
      return state;
    case "update":
      return {
        ...state,
        update_status: action.status,
        update_version: action.version,
        update_notes: action.notes ?? "",
      };
    case "nightly":
      return { ...state, nightly: action.report };
  }
}
export function position(
  playback: AppState["playback"],
  observedAt: number,
  now = Date.now(),
): number {
  return Math.max(
    0,
    Math.min(
      playback.item?.duration_ms ?? 0,
      (playback.progress_ms ?? 0) +
        (playback.is_playing ? Math.max(0, now - observedAt) : 0),
    ),
  );
}
export function activeLines(
  lines: { start_ms: number; end_ms: number; words?: Word[] }[],
  time: number,
): number[] {
  return lines.flatMap((line, index) =>
    line.start_ms <= time &&
    time <
      (line.words?.length
        ? Math.max(...line.words.map((word) => word.end_ms))
        : line.end_ms)
      ? [index]
      : [],
  );
}
/**
 * Whether the last lead line sits on the right, so credits can follow the
 * last singer. Duet lines sit right unless the sides are mirrored by Flip
 * duet sides or Right align lyrics.
 */
export function endsOnRight(
  lines: { background: boolean; opposite: boolean }[],
  mirrored: boolean,
): boolean {
  const last = lines.filter((line) => !line.background).at(-1);
  return !!last && last.opposite !== mirrored;
}
export function progress(start: number, end: number, time: number): number {
  return Math.max(0, Math.min(1, (time - start) / Math.max(1, end - start)));
}
export function formatTime(milliseconds: number): string {
  const seconds = Math.floor(Math.max(0, milliseconds) / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
