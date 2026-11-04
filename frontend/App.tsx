import { IS_MAC, MODIFIER } from "./platform";
import { useMacMenu } from "./mac-menu";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { DJRequest } from "./DJRequest";
import { createDJReminder } from "./djHint";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  Settings as SettingsIcon,
  ChevronDown,
  ChevronUp,
  Clock3,
  FolderPlus,
  Heart,
  Code,
  Bell,
  Home,
  ImagePlus,
  Pencil,
  Library,
  LoaderCircle,
  Maximize2,
  PanelRight,
  CirclePlus,
  CircleCheck,
  Mic2,
  Minus,
  MoreHorizontal,
  Music2,
  Pause,
  Play,
  Repeat,
  Repeat1,
  Search,
  Shuffle,
  SkipBack,
  SkipForward,
  Speaker,
  Square,
  Volume2,
  VolumeX,
  X,
  ListMusic,
  ExternalLink,
  Pin,
  ChevronRight,
  ChevronLeft,
  Trash2,
  Folder,
  Copy,
  Plus,
  Users,
} from "lucide-react";
import { rpc, subscribe } from "./bridge";
import { isJamLink } from "./jam-link";
import { listen } from "@tauri-apps/api/event";
import type { CinemaRequest } from "./cinema";
import { startIdleTrim } from "./memory-trim";
import { useRowWindow } from "./track-window";
import { animateLayout } from "./layout-transition";
import {
  CinemaIcon,
  PopoutIcon,
  CompactIcon,
  FullScreenIcon,
  HideArtworkIcon,
  RetryIcon,
  SwapSidesIcon,
  TranslateIcon,
  UploadIcon,
  WordSyncIcon,
} from "./toolbar-icons";
import { HomePage, Shelf } from "./HomePage";
import { SearchPage } from "./SearchPage";
import { SpotifyText } from "./SpotifyText";
import { Logo, MicLoader } from "./brand";
import { playingRoute } from "./playing-context";
import {
  emptyState,
  reducer,
  formatTime,
  nextToggle,
  sessionScreen,
  type ToggleIntent,
} from "./state";
import type { AppState } from "./state";
import type {
  FolderRef,
  Album,
  Artist,
  PlayerRequest,
  Playlist,
  Settings,
  Snapshot,
  Track,
  Jam,
  HomeItem,
  AppEvent,
  Profile,
} from "./types";
import {
  Ambience,
  ArtistLinks,
  Cover,
  titleSize,
  LyricsPage,
  Marquee,
  NowPlaying,
  QueuePanel,
  DJPresentation,
  DJArtwork,
  TrackTitle,
  artistNames,
  imageOf,
  preloadCanvas,
  prefetchCanvas,
  prefetchPage,
  cancelPrefetch,
  useLoadedImage,
  useStickyLoadedImage,
  preloadImage,
  ExplicitBadge,
  VerifiedBadge,
  usePosition,
} from "./components";
import "./styles.css";
import "./pages.css";
import "./settings.css";
import "./glass.css";
import { VolumeControl } from "./volume-control";
import { DJButton } from "./dj-button";
import {
  DJ_ID,
  accountOrder,
  customOrder,
  libraryItems,
  playlistForManualPlay,
  recordManualPlay,
  saveAccountOrder,
  togglePlaylistPin,
  forgetPlaylist,
} from "./library-order";
import { useAppFonts } from "./custom-fonts";
import { UpdatePrompt } from "./update-prompt";
import { NightlyButton } from "./nightly-button";
import { NightlyDialog } from "./nightly-dialog";
import { GlassBackdrop } from "./liquid-glass";
import { setWindowActivity, setWindowPosition } from "./window-activity";
import { useArtworkColor } from "./artwork-color";
import { SongSortMenu } from "./song-sort-menu";
import { ProfileMenu } from "./profile-menu";
import { ProfilePage } from "./profile-page";
import { SetupAmbience } from "./setup-ambience";
import { coverJpeg } from "./playlist-cover";
import {
  embedCode,
  inChosenFolders,
  songAndArtist,
  trackIds,
} from "./spotify-plus";
import {
  lastSettingsTab,
  SettingsWindow,
  type SettingsTab,
} from "./settings-window";
import { AppMenu, type AppMenuGroup } from "./app-menu";
import { AboutDialog } from "./about-dialog";
import { presetOf } from "./animator-tuning";
import { setFrameCap } from "./frame-cap";
import { AnimatorPanel } from "./animator-settings";
import {
  NextIcon,
  PauseIcon,
  PlayIcon,
  PreviousIcon,
  RepeatIcon,
  ShuffleIcon,
} from "./player-icons";
import { GlassRefractionFilter } from "./glass-refraction";
import { RecentsPage } from "./recents-page";
import { WhatsNewPage } from "./whats-new-page";
import { DiscographyPage } from "./discography-page";
import {
  addedLabel,
  collectionQueue,
  collectionTracks,
  genresFor,
  nextHeaderSort,
  playable,
} from "./collection";
import {
  albumKind,
  albumMatches,
  albumTypeLabel,
  discographyReleases,
} from "./album-type";
import { useAppZoom } from "./app-zoom";
import { PlaylistFlyout } from "./playlist-flyout";
import { LoadTtml, type LoadMode, type SavedLyrics } from "./load-ttml";
import { libraryTree, type LibraryEntry } from "./library-tree";
import {
  entryAdded,
  entryRecency,
  sortLibrary,
  type LibraryFacts,
  type Recency,
} from "./library-recents";
import { LibrarySortMenu } from "./library-sort-menu";
import {
  loadRecentSearches,
  rememberSearch,
  saveRecentSearches,
  type RecentSearch,
} from "./recent-searches";
import type { SearchEntry } from "./search-results";
/** Lyrics use Inter unless the user picks a lyrics font. */
/** Inputs that take no typing, so Space still toggles playback. */
const NON_TEXT_INPUTS = new Set([
  "range",
  "checkbox",
  "radio",
  "button",
  "submit",
  "reset",
  "color",
  "file",
]);

/** Whether a key event belongs to typing rather than the app's shortcuts. */
const isTextInput = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (!!target.closest('textarea,select,[contenteditable="true"]') ||
    (target instanceof HTMLInputElement && !NON_TEXT_INPUTS.has(target.type)));

type Center = "browse" | "lyrics" | "profile";
type EntityMenu = {
  kind: "playlist" | "album" | "artist" | "liked";
  id: string;
  name: string;
  x: number;
  y: number;
};
const windowHandle = () => getCurrentWindow();
/** What the lyrics toolbar's cache button clears before fetching again. */
const CACHE_ACTIONS = {
  song: {
    title: "Refresh lyrics (clears this song's saved copy)",
  },
  reload: {
    title: "Reload lyrics (keeps saved copies)",
  },
  all: {
    title: "Refresh lyrics (clears every saved copy)",
  },
} as const;
/** How long the lyrics take to slide away when full screen closes. */
const FULLSCREEN_EXIT_MS = 450;
/** Fit menus using their rendered size, including newly added actions. */
const keepOnScreen = (menu: HTMLElement | null, x: number, y: number) => {
  if (!menu) return;
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(8, Math.min(x, window.innerWidth - width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, window.innerHeight - height - 8))}px`;
};
/** Spotify-style total length: "1 hr 42 min" or "38 min 12 sec". */
const longDuration = (ms: number) => {
  const minutes = Math.floor(ms / 60_000);
  return minutes >= 60
    ? `${Math.floor(minutes / 60)} hr ${minutes % 60} min`
    : `${minutes} min ${Math.floor(ms / 1000) % 60} sec`;
};
/** Exact [song count, length] per list, kept so revisits show it at once. */
const listDurations = new Map<string, [number, number]>();

/** The Jam to show: `next`, unless it is an older view of the same Jam,
 * as a reply sent before Spotify's latest change report can be. */
const newerJam = (shown: Jam | null, next: Jam | null) =>
  shown &&
  next &&
  shown.session_id === next.session_id &&
  next.timestamp < shown.timestamp
    ? shown
    : next;

function App() {
  const [djRequest, setDJRequest] = useState(false);
  const [entityMenu, setEntityMenu] = useState<EntityMenu | null>(null);
  const [state, dispatch] = useReducer(reducer, emptyState);
  useEffect(() => setDJRequest(false), [state.profile?.id, state.native_ready]);
  const [center, setCenter] = useState<Center>("browse");
  const [canvasInLyrics, setCanvasInLyrics] = useState(false);
  const plus = state.settings.spotify_plus ?? {};
  const copyText = (text: string, done: string) =>
    void navigator.clipboard
      .writeText(text)
      .then(() => notify(done))
      .catch(() => notify("Could not copy that.", true));
  // Like Spotify, the window title names the playing song so external
  // tools can read it.
  const windowTitle =
    state.profile && state.playback.is_playing && state.playback.item
      ? `${state.playback.item.name} - ${state.playback.item.artists
          .map((artist) => artist.name)
          .join(", ")}`
      : "Lyssna";
  useEffect(() => {
    document.title = windowTitle;
    void windowHandle()
      .setTitle(windowTitle)
      .catch(() => {});
  }, [windowTitle]);
  // The profile on screen, and the profiles visited before it for Back.
  const [profileView, setProfileView] = useState<{
    user: string;
    hint?: Profile;
  }>();
  const profileTrail = useRef<{ user: string; hint?: Profile }[]>([]);
  // Another account never sees the previous one's profile trail.
  useEffect(() => {
    profileTrail.current = [];
    setProfileView(undefined);
    setCenter((current) => (current === "profile" ? "browse" : current));
  }, [state.profile?.id]);
  const [query, setQuery] = useState("");
  const [searchFocused, setSearchFocused] = useState(false);
  const [recentSearches, setRecentSearches] = useState<RecentSearch[]>([]);
  const [libraryQuery, setLibraryQuery] = useState("");
  // No chip by default: Your Library lists everything, as in Spotify.
  const [libraryFilter, setLibraryFilter] = useState("");
  const [collectionReload, setCollectionReload] = useState(0);
  // Saved albums and followed artists, shown unless a playlist chip is on.
  const [savedCollection, setSavedCollection] = useState<{
    albums: Album[];
    artists: Artist[];
    recency: Recency;
  } | null>(null);
  // The Spotify folders opened in Your Library, outermost first.
  const [folderPath, setFolderPath] = useState<string[]>([]);
  // Where a press on the empty top bar began, until it becomes a drag.
  const dragStart = useRef<{ x: number; y: number } | null>(null);
  // Now playing stays open or closed across restarts.
  const sidebar = state.settings.now_playing_open !== false;
  const setSidebar = (open: boolean) =>
    updateSettings({ now_playing_open: open });
  // The right panel shows Now playing or the queue.
  const [panel, setPanel] = useState<"now" | "queue">("now");
  // The Spotify Jam this computer is in, shown above the queue.
  const [jam, setJam] = useState<Jam | null>(null);
  const [jamInvite, setJamInvite] = useState(false);
  // Join a Jam from a link made by Spotify or Lyssna.
  const joinJam = (link: string) =>
    rpc<Jam>("jam", { action: "join", link: link.trim() })
      .then((joined) => {
        setJam(joined);
        setJamInvite(false);
        setPanel("queue");
        setSidebar(true);
        const host = joined.members.find(
          (member) => member.id === joined.owner_id,
        );
        notify(
          host
            ? `You joined ${host.display_name || host.username}'s Jam.`
            : "You joined the Jam.",
        );
      })
      .catch((error) => notify(String(error), true));
  const joinJamRef = useRef(joinJam);
  joinJamRef.current = joinJam;
  // Pasting a Jam link anywhere joins it.
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      const text = event.clipboardData?.getData("text") ?? "";
      if (!isJamLink(text)) return;
      event.preventDefault();
      void joinJamRef.current(text);
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, []);
  // Find a Jam already running after sign-in, then follow guests joining and
  // leaving while one is on.
  const jamOn = !!jam;
  useEffect(() => {
    if (!state.profile || !state.native_ready) {
      setJam(null);
      return;
    }
    let live = true;
    const check = () =>
      rpc<Jam | null>("jam", { action: "current" })
        .then((current) => live && setJam((shown) => newerJam(shown, current)))
        .catch(() => {});
    if (!jamOn) {
      void check();
      return () => {
        live = false;
      };
    }
    const timer = window.setInterval(check, 5000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [state.profile?.id, state.native_ready, jamOn]);
  // Spotify reports guests joining and leaving and the Jam ending at once,
  // without waiting for the next check.
  const jamRevision = state.jam_revision ?? 0;
  useEffect(() => {
    if (!jamRevision || !state.profile || !state.native_ready) return;
    // The report names who is in the Jam now, as Spotify's app shows it;
    // asking again fills in the rest.
    const report = state.jam_report;
    if (report)
      setJam((shown) =>
        shown && shown.session_id === report.session_id
          ? newerJam(shown, {
              ...shown,
              owner_id: report.owner_id || shown.owner_id,
              is_owner: report.is_owner,
              members: report.members,
              timestamp: report.timestamp,
            })
          : shown,
      );
    let live = true;
    rpc<Jam | null>("jam", { action: "current" })
      .then((current) => live && setJam((shown) => newerJam(shown, current)))
      .catch(() => {});
    return () => {
      live = false;
    };
    // Only a new report asks again; sign-in changes are covered above.
  }, [jamRevision]);
  const toggleQueue = () => {
    if (sidebar && panel === "queue") setPanel("now");
    else {
      setPanel("queue");
      setSidebar(true);
    }
  };
  // Panel widths while dragging; once let go they are saved in settings,
  // so they survive restarts and rebuilds. Library null keeps the default
  // responsive width.
  const [rightDrag, setRightDrag] = useState<number | null>(null);
  const [leftDrag, setLeftDrag] = useState<number | null>(null);
  const savedRight = state.settings.now_playing_width;
  const savedLeft = state.settings.library_width;
  const rightWidth =
    rightDrag ?? (savedRight ? Math.max(300, Math.min(520, savedRight)) : 360);
  const leftWidth =
    leftDrag ?? (savedLeft ? Math.max(250, Math.min(480, savedLeft)) : null);
  const [target, setTarget] = useState("native");
  const [deviceMenu, setDeviceMenu] = useState(false);
  // Controls follow the device Spotify reports active, so a failed
  // transfer does not leave them aimed at an unreachable speaker.
  useEffect(() => {
    const active = state.devices.find((device) => device.is_active);
    if (!active) return;
    setTarget(
      active.name === state.local_device_name || !active.id
        ? "native"
        : active.id,
    );
  }, [state.devices, state.local_device_name]);
  useEffect(() => {
    if (!deviceMenu) return;
    // Clicking anywhere outside the device popup closes it. The Devices
    // button toggles it itself, so it is left out here.
    const outside = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (
        !target?.closest('.device-menu, [title="Devices"], .playing-elsewhere')
      )
        setDeviceMenu(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [deviceMenu]);
  const [picker, setPicker] = useState<{
    tracks: Track[];
    x: number;
    y: number;
  } | null>(null);
  const [folderDialog, setFolderDialog] = useState(false);
  const [folderMenu, setFolderMenu] = useState<{
    folder: FolderRef;
    x: number;
    y: number;
  } | null>(null);
  const [folderPicker, setFolderPicker] = useState<{
    playlist: string;
    x: number;
    y: number;
  } | null>(null);
  const folderPlaylist = state.playlists.find(
    (playlist) => playlist.id === folderPicker?.playlist,
  );
  const [renameFolder, setRenameFolder] = useState<{
    folder: FolderRef;
    name: string;
  } | null>(null);
  const [deleteFolder, setDeleteFolder] = useState<FolderRef | null>(null);
  const [editDialog, setEditDialog] = useState<{
    playlist: Playlist;
    name: string;
    description: string;
    /** New cover as a JPEG data URL, once the user picks one. */
    cover?: string;
  } | null>(null);
  // Covers just saved, shown at once while Spotify takes a few seconds.
  const [coverPreviews, setCoverPreviews] = useState<Record<string, string>>(
    {},
  );
  // Row being dragged and where it would land, for playlist reordering.
  const dragRow = useRef<number | null>(null);
  const [dropAt, setDropAt] = useState<number | null>(null);
  const [removeDialog, setRemoveDialog] = useState<{
    id: string;
    name: string;
    own: boolean;
  } | null>(null);
  const [folderName, setFolderName] = useState("");
  useEffect(() => {
    setEntityMenu(null);
    setFolderDialog(false);
    setFolderMenu(null);
    setRenameFolder(null);
    setFolderPicker(null);
    setDeleteFolder(null);
  }, [state.profile?.id]);
  // Songs highlighted in the list, as in Spotify: click, Ctrl/Cmd-click and
  // Shift-click select, and nothing counts them.
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const selectAnchor = useRef<number | null>(null);
  const [songMenu, setSongMenu] = useState<{
    track: Track;
    /** The selected songs, when the menu opened on a multi-song selection. */
    tracks?: Track[];
    /** Opened on a song the listener queued, which it can remove. */
    queued?: boolean;
    fromList?: boolean;
    x: number;
    y: number;
  } | null>(null);
  const [immersive, setImmersive] = useState(false);
  const fullscreenBusy = useRef(false);
  const [albumFilter, setAlbumFilter] = useState("all");
  const [allAlbums, setAllAlbums] = useState(false);
  const [popularMore, setPopularMore] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [ttmlOpen, setTtmlOpen] = useState<false | "load" | "database">(false);
  // The Lyssna settings window and the tab it opened on.
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [animatorPanel, setAnimatorPanel] = useState(false);
  // Manage enters full screen only for tuning; closing leaves it again
  // unless full screen was already on.
  const animatorFullscreen = useRef(false);
  useEffect(() => setSettingsTab(null), [state.profile?.id]);
  const [about, setAbout] = useState(false);
  // Let WebView2 drop its caches once the user rests after browsing.
  useEffect(
    () => startIdleTrim(() => void rpc("trim_memory").catch(() => {})),
    [],
  );
  const [aboutPhoto, setAboutPhoto] = useState(0);
  // The artist header has scrolled away; show the compact sticky bar.
  const [heroGone, setHeroGone] = useState(false);
  const heroRef = useRef<HTMLElement>(null);
  const [songSearch, setSongSearch] = useState(false);
  const [songQuery, setSongQuery] = useState(""),
    [songGenre, setSongGenre] = useState(""),
    [songAge, setSongAge] = useState("");
  // Each page remembers its own sort across restarts, like Spotify.
  const songSort = state.settings.song_sorts?.[state.route] ?? "order";
  // Random reshuffles every launch, when a page is opened and when picked
  // again. Coming back to a page with Back keeps its order.
  const shuffleSeeds = useRef(new Map<string, number>());
  const [, reshuffled] = useState(0);
  let shuffleSeed = shuffleSeeds.current.get(state.route);
  if (shuffleSeed === undefined) {
    shuffleSeed = Math.random() * 2 ** 32;
    shuffleSeeds.current.set(state.route, shuffleSeed);
  }
  const setSongSort = (sort: string) => {
    if (sort === "random") {
      shuffleSeeds.current.delete(state.route);
      reshuffled((count) => count + 1);
    }
    const song_sorts = { ...state.settings.song_sorts };
    if (sort === "order") delete song_sorts[state.route];
    else song_sorts[state.route] = sort;
    updateSettings({ song_sorts });
  };
  useEffect(() => {
    setSongSearch(false);
    setSongQuery("");
    setSongGenre("");
    setSongAge("");
  }, [state.route]);
  const generation = useRef(0),
    bootRoute = useRef("home"),
    loadedAccount = useRef(""),
    history = useRef<string[]>([]),
    future = useRef<string[]>([]),
    routeRef = useRef("home"),
    stateRef = useRef(state),
    fileInput = useRef<HTMLInputElement>(null),
    searchInput = useRef<HTMLInputElement>(null),
    mainScroll = useRef<HTMLDivElement>(null),
    trackTable = useRef<HTMLDivElement>(null),
    // Liked Songs started from the sidebar plays once its songs arrive.
    likedPlay = useRef(false),
    pageSentinel = useRef<HTMLDivElement>(null),
    pendingPage = useRef<string>("");
  stateRef.current = state;
  const fonts = useAppFonts(state.settings);
  routeRef.current = state.route;
  const notify = useCallback(
    (message: string, error = false) =>
      dispatch({ type: "notice", message, error }),
    [],
  );
  const djReminder = useMemo(
    () =>
      createDJReminder({
        getItem: (key) => window.localStorage.getItem(key),
        setItem: (key, value) => window.localStorage.setItem(key, value),
      }),
    [],
  );
  useEffect(() => {
    if (state.narrating && state.profile && djReminder(state.profile.id))
      notify("Hold the DJ button to ask for music.");
  }, [state.narrating, state.profile, djReminder, notify]);
  const call = useCallback(
    async <T,>(
      command: string,
      args?: Record<string, unknown>,
    ): Promise<T | undefined> => {
      try {
        return await rpc<T>(command, args);
      } catch (error) {
        notify(String(error), true);
        return undefined;
      }
    },
    [notify],
  );
  // A Nightly copy gets a button that looks for newer Nightly code.
  const [nightlyCommit, setNightlyCommit] = useState<string | null>(null);
  const [linkSignal, setLinkSignal] = useState(0);
  // The first Nightly build, started from Settings, shows on the top bar
  // from the moment it is confirmed; clicking it shows its progress.
  const switchingToNightly = !nightlyCommit && !!state.nightly?.running;
  const [firstBuildOpen, setFirstBuildOpen] = useState(false);
  const [checkingNightly, setCheckingNightly] = useState(false);
  useEffect(() => {
    void rpc<{ commit: string | null }>("nightly_status")
      .then((status) => setNightlyCommit(status?.commit ?? null))
      .catch(() => setNightlyCommit(null));
  }, []);
  const checkNightly = useCallback(async () => {
    setCheckingNightly(true);
    const found = await call<string>("nightly_check");
    setCheckingNightly(false);
    if (found === "current") notify("You're on the latest Nightly.");
    else if (found === "available" || found === "building" || found === "ready")
      window.dispatchEvent(new Event("lyssna:show-update"));
    else if (found) notify(found);
  }, [call, notify]);
  const navigate = useCallback(
    (key: string, push = true) => {
      if (!stateRef.current.profile) return;
      // Opening a page shuffles Random anew; Back and Forward keep it.
      if (push) shuffleSeeds.current.delete(key);
      if (push && routeRef.current !== key) {
        if (!(
          routeRef.current.startsWith("search:") && key.startsWith("search:")
        ))
          history.current.push(routeRef.current);
        future.current = [];
      }
      setCenter("browse");
      setAlbumFilter("all");
      setAllAlbums(false);
      setPopularMore(false);
      setSongMenu(null);
      setSelected(new Set());
      selectAnchor.current = null;
      const next = ++generation.current;
      const hint = stateRef.current.home?.sections
        .flatMap((section) => section.items)
        .find((item) => item.route === key);
      if (hint && (key.startsWith("artist:") || key.startsWith("album:")))
        dispatch({
          type: "info_prefetch",
          info: {
            key,
            title: hint.name,
            subtitle: hint.subtitle,
            images: hint.images,
            albums: [],
          },
        });
      openedAt.current = { key, at: performance.now() };
      dispatch({ type: "navigate", key, generation: next });
      mainScroll.current?.scrollTo({ top: 0 });
      void call("browse", { key, generation: next, offset: 0 });
    },
    [call],
  );
  const navigateRef = useRef(navigate);
  /** When the open page was clicked, for the diagnostics log. */
  const openedAt = useRef<{ key: string; at: number } | null>(null);
  useEffect(() => {
    const opened = openedAt.current;
    if (!opened || opened.key !== state.route || !state.tracks.length) return;
    openedAt.current = null;
    const songs = state.tracks.length;
    // After the list is painted.
    requestAnimationFrame(() =>
      setTimeout(
        () =>
          void rpc("page_shown", {
            kind: opened.key,
            songs,
            ms: Math.round(performance.now() - opened.at),
          }).catch(() => {}),
      ),
    );
  }, [state.route, state.tracks]);
  /** A song a `lyssna://` link named, to select once its album lists it. */
  const linkedSong = useRef<string | null>(null);
  navigateRef.current = navigate;
  // Songs found in Local Files folders, for Your Library's row.
  const [localCount, setLocalCount] = useState(0);
  const hasProfile = !!state.profile;
  useEffect(() => {
    if (!hasProfile) return;
    rpc<number>("local_files_count").then(
      (count) => setLocalCount(count ?? 0),
      () => undefined,
    );
  }, [hasProfile]);
  useEffect(() => {
    let disposed = false,
      unlisten: (() => void) | undefined;
    void (async () => {
      let bootstrapped = false;
      const pending: AppEvent[] = [];
      const receive = (event: AppEvent) => {
        if (event.type === "window_activity") {
          setWindowActivity(event);
          return;
        }
        if (event.type === "window_moved") {
          setWindowPosition(event.x, event.y);
          return;
        }
        // Listen on a creator's profile plays the song here.
        if (event.type === "play_uri") {
          playRef.current?.({ command: "play", uri: event.uri });
          return;
        }
        // A `lyssna://` link: taken below once signed in and ready.
        if (event.type === "link") {
          setLinkSignal((count) => count + 1);
          return;
        }
        // A folder scan finished: count the row and refresh the open page.
        if (event.type === "local_files") {
          setLocalCount(event.count);
          if (routeRef.current === "local") navigateRef.current("local", false);
          return;
        }
        if (event.type === "canvas_prefetch") preloadCanvas(event.url);
        // Upcoming artists' About photos, decoded before their songs start.
        if (event.type === "artist_about")
          void preloadImage(
            event.about.gallery_image || event.about.image || undefined,
          );
        if (event.type === "session")
          void preloadImage(imageOf(event.profile.images));
        if (event.type === "browse_info" || event.type === "info_prefetch") {
          void preloadImage(imageOf(event.info.images));
          void preloadImage(event.info.overview?.header_image);
        }
        dispatch(event);
      };
      try {
        unlisten = await subscribe((event) => {
          if (disposed) return;
          if (!bootstrapped) {
            pending.push(event);
            return;
          }
          receive(event);
        });
        const snapshot = await rpc<Snapshot>("bootstrap");
        void preloadImage(imageOf(snapshot.profile?.images));
        void preloadImage(snapshot.browse_info?.overview?.header_image);
        if (!disposed) {
          generation.current = snapshot.generation;
          bootRoute.current = snapshot.route || "home";
          setQuery(
            snapshot.route.startsWith("search:") ? snapshot.route.slice(7) : "",
          );
          dispatch({ type: "bootstrap", snapshot });
          bootstrapped = true;
          pending.splice(0).forEach(receive);
        }
      } catch (error) {
        if (!disposed)
          notify(`Cannot connect to desktop services: ${String(error)}`, true);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [notify]);
  useEffect(() => {
    if (state.profile && loadedAccount.current !== state.profile.id) {
      loadedAccount.current = state.profile.id;
      navigate(bootRoute.current, false);
      void call("refresh");
    }
    if (!state.profile) loadedAccount.current = "";
  }, [state.profile, navigate, call]);
  useEffect(() => {
    if (!state.profile || !query.trim()) return;
    const requestGeneration = generation.current;
    const timer = setTimeout(() => {
      if (generation.current !== requestGeneration) return;
      const key = `search:${query.trim()}`;
      if (routeRef.current !== key) navigate(key);
    }, 250);
    return () => clearTimeout(timer);
  }, [query, state.profile?.id, navigate]);
  useEffect(() => {
    if (state.toast) {
      const timer = setTimeout(() => dispatch({ type: "toast_clear" }), 6000);
      return () => clearTimeout(timer);
    }
  }, [state.toast]);
  const updateSettings = useCallback(
    (changes: Partial<Settings>) => {
      const settings = { ...stateRef.current.settings, ...changes };
      stateRef.current = { ...stateRef.current, settings };
      dispatch({ type: "settings", settings });
      void call("save_settings", { settings });
    },
    [call],
  );
  // Styles that differ between animator presets key off the root.
  const cacheAction = state.settings.lyrics_cache_action ?? "song";
  useEffect(() => {
    setFrameCap(
      state.settings.animation_fps_cap_enabled
        ? (state.settings.animation_fps_cap ?? 60)
        : 0,
    );
  }, [
    state.settings.animation_fps_cap_enabled,
    state.settings.animation_fps_cap,
  ]);
  const animatorPreset = presetOf(state.settings.lyrics_animator);
  useEffect(() => {
    document.documentElement.dataset.animator = animatorPreset;
  }, [animatorPreset]);
  useAppZoom(
    state.settings.ui_zoom_percent ?? 100,
    (percent) => updateSettings({ ui_zoom_percent: percent }),
    notify,
  );
  const togglePin = useCallback(
    (playlist: Playlist) => {
      try {
        const current = stateRef.current;
        if (!current.profile) return;
        updateSettings(
          saveAccountOrder(
            current,
            togglePlaylistPin(accountOrder(current), playlist),
          ),
        );
      } catch (error) {
        notify(
          error instanceof Error ? error.message : "Could not pin playlist",
          true,
        );
      }
    },
    [notify, updateSettings],
  );
  const songMenuElement = useRef<HTMLDivElement>(null);
  const entityMenuElement = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (!entityMenu && !songMenu) return;
    const position = () => {
      if (songMenu)
        keepOnScreen(songMenuElement.current, songMenu.x, songMenu.y);
      if (entityMenu)
        keepOnScreen(entityMenuElement.current, entityMenu.x, entityMenu.y);
    };
    position();
    document.querySelector<HTMLButtonElement>(".song-menu button")?.focus();
    const close = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setEntityMenu(null);
        setSongMenu(null);
      }
    };
    window.addEventListener("resize", position);
    window.addEventListener("keydown", close);
    return () => {
      window.removeEventListener("resize", position);
      window.removeEventListener("keydown", close);
    };
  }, [entityMenu, songMenu]);
  const toggleIntent = useRef<ToggleIntent | undefined>(undefined);
  const refreshHome = useCallback(() => {
    if (!stateRef.current.profile || !stateRef.current.native_ready) return;
    void call("home", {
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
    });
  }, [call]);
  useEffect(() => {
    if (
      state.profile &&
      state.native_ready &&
      center === "browse" &&
      state.route === "home"
    )
      refreshHome();
    // Home refreshes on entry and return, as Spotify's does; skipping songs
    // while it is open no longer reloads its shelves.
  }, [state.profile?.id, state.native_ready, center, state.route, refreshHome]);
  // Custom animator tuning sits beside full screen lyrics; Settings and the
  // Cinema window both open it.
  const manageAnimator = useRef(() => {});
  // The Cinema window hands over what lives here: opening a page, importing
  // lyrics and settings. The main window comes forward to show it.
  useEffect(() => {
    let disposed = false,
      unlisten: (() => void) | undefined;
    void listen<CinemaRequest>("cinema-request", ({ payload }) => {
      const window = windowHandle();
      void window.unminimize().then(() => window.setFocus());
      if (payload.kind === "route") navigate(payload.route);
      else if (payload.kind === "import") setTtmlOpen("load");
      else if (payload.kind === "database") setTtmlOpen("database");
      else if (payload.kind === "animator") manageAnimator.current();
      else
        setSettingsTab(
          lastSettingsTab(
            stateRef.current.settings.settings_remember_place !== false,
          ),
        );
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [navigate]);
  const metadataKeys = useMemo(
    () =>
      [
        ...new Set([
          ...(state.playback.item?.artists.flatMap((artist) =>
            artist.id ? [`artist:${artist.id}`] : [],
          ) || []),
          ...(state.playback.item?.album.id
            ? [`album:${state.playback.item.album.id}`]
            : []),
          ...state.tracks
            .slice(0, 8)
            .flatMap((track) =>
              track.artists.flatMap((artist) =>
                artist.id ? [`artist:${artist.id}`] : [],
              ),
            ),
          ...(state.home?.account === state.profile?.id
            ? state.home?.sections.flatMap((section) =>
                section.items
                  .filter(
                    (item) => item.kind === "artist" || item.kind === "album",
                  )
                  .map((item) => item.route),
              ) || []
            : []),
        ]),
      ].slice(0, 8),
    [state.playback.item?.uri, state.tracks, state.home, state.profile?.id],
  );
  const metadataSignature = metadataKeys.join("|");
  // Featured Home albums show a saved mark; ask for their status.
  const featuredAlbums = (state.home?.sections ?? [])
    .filter(
      (section) =>
        section.items.length === 1 && section.items[0].kind === "album",
    )
    .map((section) => section.items[0].uri)
    .join("|");
  useEffect(() => {
    if (state.profile && state.native_ready && featuredAlbums)
      void rpc("saved_status", { uris: featuredAlbums.split("|") }).catch(
        () => {},
      );
  }, [state.profile?.id, state.native_ready, featuredAlbums]);
  useEffect(() => {
    if (state.profile && state.native_ready && metadataKeys.length)
      void rpc("prefetch_metadata", { keys: metadataKeys }).catch(() => {});
  }, [state.profile?.id, state.native_ready, metadataSignature]);
  const play = useCallback(
    (request: PlayerRequest) => {
      // The native side takes whole milliseconds; seek bars and the
      // interpolated song clock report fractions.
      if (request.positionMs !== undefined)
        request = {
          ...request,
          positionMs: Math.max(0, Math.round(request.positionMs)),
        };
      const current = stateRef.current,
        item = playlistForManualPlay(current, request);
      if (item && current.profile)
        updateSettings(
          saveAccountOrder(
            current,
            recordManualPlay(accountOrder(current), item),
          ),
        );
      if (
        request.command === "pause" ||
        (request.command === "play" && !request.uri)
      )
        toggleIntent.current = {
          wanted: request.command === "play",
          at: performance.now(),
        };
      if (request.track)
        dispatch({ type: "play_requested", track: request.track });
      if (request.command === "dj") dispatch({ type: "dj_requested" });
      rpc("player", { request, target }).catch((error) =>
        dispatch({
          type: "notice",
          message: String(error),
          error: true,
          stalled: true,
        }),
      );
    },
    [target, updateSettings],
  );
  const playRef = useRef(play);
  playRef.current = play;
  // Restarted by an update mid-song: carry on from where it stopped. Asked
  // as soon as the song is restored; the player starts it the moment it
  // connects rather than waiting for the app to say it is ready.
  const restoredUri = state.playback.item?.uri;
  useEffect(() => {
    if (!state.profile || !restoredUri) return;
    rpc<boolean>("resume_after_restart")
      .then((resume) => {
        if (resume) playRef.current({ command: "play" });
      })
      .catch(() => {});
  }, [state.profile?.id, restoredUri]);
  // A card already knows its name and picture: show them at once while
  // Spotify sends the rest of the page (about half a second for artists).
  const openCard = useCallback(
    (route: string, item?: HomeItem) => {
      const [kind, id] = route.split(":");
      if (
        item &&
        id &&
        (kind === "artist" || kind === "album") &&
        !stateRef.current.pages[route]?.info?.title
      )
        dispatch({
          type: "info_prefetch",
          info: {
            key: route,
            title: item.name,
            subtitle: kind === "artist" ? "Artist" : item.subtitle,
            images: item.images,
            albums: [],
            ...(kind === "artist"
              ? {
                  artist: {
                    id,
                    uri: item.uri,
                    name: item.name,
                    images: item.images,
                  },
                }
              : {}),
          },
        });
      navigate(route);
    },
    [navigate],
  );
  const homeNavigate = useCallback(
    (route: string, item?: HomeItem) =>
      route === "playlist:37i9dQZF1EYkqdzj48dyYq"
        ? play({ command: "dj" })
        : openCard(route, item),
    [openCard, play],
  );
  const homePlay = useCallback(
    (item: HomeItem) => {
      if (!item.playable) return;
      if (item.route === "liked") navigate("liked");
      else
        play(
          item.uri === "spotify:playlist:37i9dQZF1EYkqdzj48dyYq"
            ? { command: "dj" }
            : { command: "play_home", context: item.uri },
        );
    },
    [navigate, play],
  );
  const startDJ = useCallback(() => play({ command: "dj" }), [play]);
  const toggle = useCallback(() => {
    const intent = nextToggle(
      stateRef.current.playback.is_playing,
      toggleIntent.current,
      performance.now(),
    );
    play({ command: intent.wanted ? "play" : "pause" });
  }, [play]);
  useEffect(() => {
    if (!state.dj_pending) return;
    const timer = setTimeout(
      () => dispatch({ type: "dj_pending_expired" }),
      5000,
    );
    return () => clearTimeout(timer);
  }, [state.dj_pending]);
  // Taskbar buttons, media keys and the Windows media flyout.
  useEffect(() => {
    let unlisten: (() => void) | undefined,
      disposed = false;
    void subscribe((event) => {
      if (event.type !== "media_control" || !stateRef.current.profile) return;
      if (event.action === "toggle") toggle();
      else if (event.action === "seek") {
        if (event.position_ms != null)
          play({ command: "seek", positionMs: event.position_ms });
      } else play({ command: event.action });
    }).then((stop) => (disposed ? stop() : (unlisten = stop)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [toggle, play]);
  const item = state.playback.item;
  useEffect(() => {
    void rpc("media_state", {
      state: {
        title: state.narrating ? "DJ" : (item?.name ?? ""),
        artists: item?.artists.map((artist) => artist.name).join(", ") ?? "",
        album: item?.album.name ?? "",
        cover: item?.album.images?.[0]?.url ?? "",
        durationMs: item?.duration_ms ?? 0,
        positionMs: Math.round(state.playback.progress_ms ?? 0),
        playing: state.playback.is_playing,
        loaded: !!item,
      },
    }).catch(() => {});
  }, [
    item?.uri,
    state.narrating,
    state.playback.is_playing,
    state.clock_revision,
  ]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (document.querySelector("dialog[open]")) return;
      // Only typing keeps Space and Escape; a focused button or slider still
      // leaves Space for play and pause.
      const editing = isTextInput(event.target);
      if (
        (event.ctrlKey || event.metaKey) &&
        ["k", "l"].includes(event.key.toLowerCase())
      ) {
        event.preventDefault();
        searchInput.current?.focus();
        return;
      }
      // F5 is the one refresh, as in a browser: the library and the open
      // page, picking up changes from other devices.
      if (event.key === "F5") {
        event.preventDefault();
        if (!stateRef.current.profile) return;
        void call("refresh");
        setCollectionReload((count) => count + 1);
        if (stateRef.current.route === "home") refreshHome();
        else navigate(stateRef.current.route, false);
        return;
      }
      if (!stateRef.current.profile || editing) return;
      if (event.code === "Space") {
        // Space always plays or pauses, even on a focused button.
        event.preventDefault();
        event.stopPropagation();
        if (!event.repeat) toggle();
      }
      if (event.key === "Escape") {
        setSongMenu(null);
        setDeviceMenu(false);
        setPicker(null);
        setFolderDialog(false);
        setRemoveDialog(null);
        setEditDialog(null);
        setTtmlOpen(false);
        setFolderMenu(null);
        setRenameFolder(null);
        setAboutOpen(false);
        setFolderPicker(null);
        setDeleteFolder(null);
      }
    };
    // A focused button activates on Space keyup; cancel that so Space never
    // also presses whatever button was clicked last.
    const keyup = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (
        event.code === "Space" &&
        !target?.closest("dialog[open]") &&
        !target?.closest('textarea,select,[contenteditable="true"]') &&
        !(
          target instanceof HTMLInputElement &&
          !NON_TEXT_INPUTS.has(target.type)
        )
      )
        event.preventDefault();
    };
    // A mouse click must not leave the button focused, or Space and Enter
    // would press it again; Space is only for play and pause. Runs after
    // React's handlers, and skips buttons that moved focus elsewhere.
    const click = (event: MouseEvent) => {
      const button = (event.target as HTMLElement | null)?.closest<HTMLElement>(
        'button,[role="button"],[role="menuitem"],[role="tab"],[role="option"],summary',
      );
      if (
        event.detail > 0 &&
        button &&
        !button.closest("dialog[open]") &&
        document.activeElement === button
      )
        button.blur();
    };
    document.addEventListener("click", click);
    // Capture so no other handler can see or stop Space first.
    window.addEventListener("keydown", keydown, true);
    window.addEventListener("keyup", keyup, true);
    return () => {
      window.removeEventListener("keydown", keydown, true);
      window.removeEventListener("keyup", keyup, true);
      document.removeEventListener("click", click);
    };
  }, [toggle, call, navigate, refreshHome]);
  const prefetchOn = useCallback(
    (key: string) => ({
      onMouseEnter: () => {
        const current = stateRef.current;
        if (
          current.profile &&
          key !== current.route &&
          !current.pages[key]?.tracks
        )
          prefetchPage(key);
      },
      onMouseLeave: cancelPrefetch,
    }),
    [],
  );
  const openAlbum = useCallback(
    (album: Album) => {
      const id = album.id || album.uri?.split(":")[2];
      if (id) navigate(`album:${id}`);
      else {
        const current = stateRef.current,
          track = [current.playback.item, ...current.tracks].find(
            (track) => track?.album === album,
          );
        if (track)
          void call("resolve_album", { uri: track.uri }).then((value) => {
            const resolved = value as Album | undefined;
            const id = resolved?.id || resolved?.uri?.split(":")[2];
            if (id) navigate(`album:${id}`);
          });
        else if (album.name) setQuery(album.name);
      }
    },
    [navigate, call],
  );
  const openArtist = useCallback(
    (artist: Artist) => {
      const id = artist.id || artist.uri?.split(":")[2];
      if (id) {
        const known =
          stateRef.current.details.artists.find(
            (item) => item.id === id && item.images?.length,
          ) || artist;
        dispatch({
          type: "info_prefetch",
          info: {
            key: `artist:${id}`,
            title: artist.name,
            subtitle: "Artist",
            images: known.images || [],
            artist: known,
            albums: [],
          },
        });
        navigate(`artist:${id}`);
      } else setQuery(artist.name);
    },
    [navigate],
  );
  const follow = useCallback(
    (artist: Artist) => {
      if (!artist.id) return;
      if (!stateRef.current.can_follow) {
        setSettingsTab("General");
        notify(
          "Enable artist following by updating Spotify permissions in Settings.",
        );
        return;
      }
      const uri = artist.uri || `spotify:artist:${artist.id}`;
      void call("save_item", { uri, saved: !stateRef.current.saved[uri] });
    },
    [call, notify],
  );
  const save = useCallback(
    (track: Track) => {
      const saved = !stateRef.current.saved[track.uri];
      // Apply at once; the worker reports the old state back on failure.
      dispatch({ type: "saved", uri: track.uri, saved });
      void call("save_item", { uri: track.uri, saved });
    },
    [call],
  );
  const openEditDialog = useCallback((playlist: Playlist) => {
    const current = stateRef.current;
    setEditDialog({
      playlist,
      name: playlist.name,
      description:
        current.browse_info?.key === `playlist:${playlist.id}`
          ? current.browse_info.subtitle
          : "",
    });
  }, []);
  const moveInPlaylist = useCallback(
    (from: number, to: number) => {
      const current = stateRef.current,
        playlist = current.playlists.find(
          (item) => `playlist:${item.id}` === current.route,
        ),
        track = current.tracks[from];
      if (!playlist || !track || from === to) return;
      dispatch({ type: "track_moved", key: current.route, from, to });
      void call("move_playlist_song", {
        playlist,
        from,
        uri: track.uri,
        to,
      });
    },
    [call],
  );
  const removeFromPlaylist = useCallback(
    (track: Track) => {
      const current = stateRef.current,
        playlist = current.playlists.find(
          (item) => `playlist:${item.id}` === current.route,
        ),
        index = current.tracks.indexOf(track);
      if (!playlist || index < 0) return;
      dispatch({ type: "track_removed", key: current.route, index });
      void call("remove_from_playlist", { playlist, index, uri: track.uri });
    },
    [call],
  );
  const importFile = async (file: File, mode: LoadMode = "persistent") => {
    if (file.size > 2_000_000) {
      notify("Lyric file exceeds 2 MB", true);
      return;
    }
    const sent = await rpc("import_lyrics", {
      filename: file.name,
      contents: await file.text(),
      mode,
    }).then(
      () => true,
      (error) => {
        notify(String(error), true);
        return false;
      },
    );
    if (!sent) return;
    setTtmlOpen(false);
    notify(
      mode === "persistent"
        ? "Lyrics saved for this song."
        : mode === "session"
          ? "Lyrics applied until Lyssna restarts."
          : "Lyrics applied until they reload.",
    );
  };
  const listSavedLyrics = useCallback(
    () => rpc<SavedLyrics[]>("saved_lyrics").catch(() => []),
    [],
  );
  const back = () => {
    const previous =
      center === "profile" ? profileTrail.current.pop() : undefined;
    if (previous) {
      setProfileView(previous);
      return;
    }
    if (center !== "browse") {
      setCenter("browse");
      return;
    }
    const key = history.current.pop();
    if (key) {
      future.current.push(state.route);
      navigate(key, false);
    }
  };
  const forward = () => {
    const key = future.current.pop();
    if (key) {
      history.current.push(state.route);
      navigate(key, false);
    }
  };
  // Spotify's "⋯" menu. Edit actions act on the field that had focus.
  const edit = (command: string) => () => document.execCommand(command);
  const zoomBy = (step: number) =>
    updateSettings({
      ui_zoom_percent: Math.min(
        200,
        Math.max(75, (state.settings.ui_zoom_percent ?? 100) + step),
      ),
    });
  const songPosition = () =>
    (state.playback.progress_ms ?? 0) +
    (state.playback.is_playing ? Date.now() - state.observed_at : 0);
  const volume = state.playback.device.volume_percent ?? 100;
  const signedIn = !!state.profile;
  const menuGroups: AppMenuGroup[] = [
    {
      label: "File",
      items: [
        {
          label: "New playlist",
          disabled: !signedIn,
          onSelect: () =>
            void call("create_playlist", {
              name: `My Playlist #${
                state.playlists.filter(
                  (playlist) => playlist.owner.id === state.profile?.id,
                ).length + 1
              }`,
              tracks: [],
            }),
        },
        {
          label: "New playlist folder",
          disabled: !signedIn,
          onSelect: () => setFolderDialog(true),
        },
        {
          label: "Log out",
          divider: true,
          disabled: !signedIn,
          onSelect: () => void call("sign_out"),
        },
        { label: "Exit", onSelect: () => void rpc("quit_app").catch(() => {}) },
      ],
    },
    {
      label: "Edit",
      items: [
        { label: "Undo", shortcut: `${MODIFIER}+Z`, onSelect: edit("undo") },
        {
          label: "Redo",
          shortcut: IS_MAC ? "Cmd+Shift+Z" : "Ctrl+Y",
          onSelect: edit("redo"),
        },
        {
          label: "Cut",
          shortcut: `${MODIFIER}+X`,
          divider: true,
          onSelect: edit("cut"),
        },
        { label: "Copy", shortcut: `${MODIFIER}+C`, onSelect: edit("copy") },
        {
          label: "Paste",
          shortcut: `${MODIFIER}+V`,
          onSelect: () =>
            void navigator.clipboard
              .readText()
              .then((text) => document.execCommand("insertText", false, text))
              .catch(() => notify("Clipboard is unavailable.", true)),
        },
        { label: "Delete", shortcut: "Del", onSelect: edit("delete") },
        {
          label: "Select all",
          shortcut: `${MODIFIER}+A`,
          onSelect: edit("selectAll"),
        },
        {
          label: "Search",
          shortcut: `${MODIFIER}+K`,
          divider: true,
          onSelect: () => searchInput.current?.focus(),
        },
        {
          label: "Settings",
          onSelect: () =>
            setSettingsTab(
              lastSettingsTab(state.settings.settings_remember_place !== false),
            ),
        },
      ],
    },
    {
      label: "View",
      items: [
        {
          label: "Zoom in",
          shortcut: `${MODIFIER}+=`,
          onSelect: () => zoomBy(10),
        },
        {
          label: "Zoom out",
          shortcut: `${MODIFIER}+-`,
          onSelect: () => zoomBy(-10),
        },
        {
          label: "Reset zoom",
          shortcut: `${MODIFIER}+0`,
          onSelect: () => updateSettings({ ui_zoom_percent: 100 }),
        },
        {
          label: state.settings.compact_library
            ? "Expand Your Library"
            : "Compact Your Library",
          divider: true,
          onSelect: () =>
            updateSettings({
              compact_library: !state.settings.compact_library,
            }),
        },
        {
          label: sidebar ? "Hide Now playing view" : "Show Now playing view",
          onSelect: () => {
            setSidebar(!sidebar);
            setPanel("now");
          },
        },
        {
          label: center === "lyrics" ? "Hide lyrics" : "Lyrics",
          disabled: !signedIn,
          onSelect: () => setCenter(center === "lyrics" ? "browse" : "lyrics"),
        },
        { label: "Queue", disabled: !signedIn, onSelect: toggleQueue },
        {
          label: "Full screen",
          shortcut: "F11",
          onSelect: () => void fullscreen(),
        },
      ],
    },
    {
      label: "Playback",
      items: [
        {
          label: state.playback.is_playing ? "Pause" : "Play",
          shortcut: "Space",
          disabled: !signedIn,
          onSelect: toggle,
        },
        {
          label: "Next",
          disabled: !signedIn,
          onSelect: () => play({ command: "next" }),
        },
        {
          label: "Previous",
          disabled: !signedIn,
          onSelect: () => play({ command: "previous" }),
        },
        {
          label: "Seek forward",
          divider: true,
          disabled: !state.playback.item,
          onSelect: () =>
            play({ command: "seek", positionMs: songPosition() + 10_000 }),
        },
        {
          label: "Seek backward",
          disabled: !state.playback.item,
          onSelect: () =>
            play({
              command: "seek",
              positionMs: Math.max(0, songPosition() - 10_000),
            }),
        },
        {
          label: state.playback.shuffle_state ? "Shuffle off" : "Shuffle",
          divider: true,
          disabled: !signedIn,
          onSelect: () =>
            play({
              command: "shuffle",
              enabled: !state.playback.shuffle_state,
            }),
        },
        {
          label:
            state.playback.repeat_state === "track"
              ? "Repeat off"
              : state.playback.repeat_state === "context"
                ? "Repeat one"
                : "Repeat",
          disabled: !signedIn,
          onSelect: () =>
            play({
              command: "repeat",
              repeat:
                state.playback.repeat_state === "off" ||
                !state.playback.repeat_state
                  ? "context"
                  : state.playback.repeat_state === "context"
                    ? "track"
                    : "off",
            }),
        },
        {
          label: "Volume up",
          divider: true,
          disabled: !signedIn,
          onSelect: () =>
            play({ command: "volume", volume: Math.min(100, volume + 10) }),
        },
        {
          label: "Volume down",
          disabled: !signedIn,
          onSelect: () =>
            play({ command: "volume", volume: Math.max(0, volume - 10) }),
        },
      ],
    },
    {
      label: "Help",
      items: [
        {
          label: "About Lyssna",
          onSelect: () => setAbout(true),
        },
        {
          label: "Check for updates",
          disabled: IS_MAC,
          onSelect: () => void call("app_update", { action: "check" }),
        },
        {
          label: "Keyboard shortcuts",
          onSelect: () =>
            notify(
              `Space play or pause · ${MODIFIER}+K search · ${MODIFIER}+= / ${MODIFIER}+- / ${MODIFIER}+0 zoom · F5 refresh · F11 full screen · Esc close menus`,
            ),
        },
      ],
    },
  ];
  const nativeMenuReady = useMacMenu(menuGroups, (message) =>
    notify(message, true),
  );
  const fullscreen = async (force?: boolean) => {
    if (fullscreenBusy.current) return;
    fullscreenBusy.current = true;
    // Animate closing full screen: the lyrics slide down out of view first.
    let leaving: HTMLElement | null = null;
    try {
      const next = force ?? !(await windowHandle().isFullscreen());
      leaving =
        !next &&
        state.settings.fullscreen_animate_close &&
        !state.settings.reduce_motion
          ? document.querySelector<HTMLElement>(".immersive .lyrics-page")
          : null;
      if (leaving) {
        leaving.classList.add("fullscreen-exit");
        await new Promise((resolve) => setTimeout(resolve, FULLSCREEN_EXIT_MS));
      }
      // The backend leaves and restores maximized without the window
      // shrinking to its restore size on the way.
      await rpc("window_fullscreen", { enabled: next });
      setImmersive(next);
      if (next) setCenter("lyrics");
    } catch (error) {
      notify(String(error), true);
    } finally {
      leaving?.classList.remove("fullscreen-exit");
      fullscreenBusy.current = false;
    }
  };
  manageAnimator.current = () => {
    setAnimatorPanel(true);
    animatorFullscreen.current = !immersive;
    if (!immersive) void fullscreen(true);
    else setCenter("lyrics");
  };
  useEffect(() => {
    void windowHandle()
      .isFullscreen()
      .then((value) => setImmersive(!!value));
  }, []);
  const closeAnimatorPanel = () => {
    setAnimatorPanel(false);
    if (animatorFullscreen.current) void fullscreen(false);
    animatorFullscreen.current = false;
  };
  useEffect(() => {
    // Escape Key Function: Exit fully also leaves the lyrics page, in or
    // out of full screen, unless a dialog, menu or text field has Escape.
    // Checked while capturing, before a dialog's own Escape removes it.
    const exitFully = !!state.settings.lyrics_escape_exits;
    const handler = (event: KeyboardEvent) => {
      if (event.key === "F12" && state.settings.developer_mode) {
        event.preventDefault();
        void call("open_devtools");
        return;
      }
      if (event.key === "F11" || (event.key === "Escape" && immersive)) {
        event.preventDefault();
        void fullscreen(event.key === "Escape" ? false : undefined);
        if (event.key === "Escape" && exitFully) setCenter("browse");
      }
    };
    const leave = (event: KeyboardEvent) => {
      if (
        event.key === "Escape" &&
        exitFully &&
        !immersive &&
        center === "lyrics" &&
        !isTextInput(event.target) &&
        !document.querySelector('dialog[open], [role="dialog"], [role="menu"]')
      )
        setCenter("browse");
    };
    window.addEventListener("keydown", handler);
    window.addEventListener("keydown", leave, true);
    return () => {
      window.removeEventListener("keydown", handler);
      window.removeEventListener("keydown", leave, true);
    };
  }, [
    immersive,
    center,
    state.settings.lyrics_escape_exits,
    state.settings.developer_mode,
  ]);
  const playlist = state.playlists.find(
    (playlist) => state.route === `playlist:${playlist.id}`,
  );
  const albumFallback = state.route.startsWith("album:")
    ? state.tracks.find((track) => track.album.id === state.route.slice(6))
        ?.album
    : undefined;
  const fallbackInfo: typeof state.browse_info = albumFallback
    ? {
        key: state.route,
        title: albumFallback.name,
        subtitle: "",
        images: albumFallback.images,
        album: albumFallback,
        albums: [],
      }
    : undefined;
  const entity: typeof state.browse_info = state.browse_info
    ? {
        ...fallbackInfo,
        ...state.browse_info,
        title: state.browse_info.title || fallbackInfo?.title || "",
        images: state.browse_info.images?.length
          ? state.browse_info.images
          : (fallbackInfo?.images ?? []),
        album: state.browse_info.album ?? fallbackInfo?.album,
      }
    : fallbackInfo;
  const artistBanner = useStickyLoadedImage(
    entity?.overview?.header_image,
    state.route,
  );
  const artistPage = state.route.startsWith("artist:");
  const artistFallback =
    artistPage && (!entity?.overview?.header_image || !artistBanner);
  const artistVerified = entity?.overview?.verified ?? entity?.artist?.verified;
  const monthlyListeners =
    entity?.overview?.monthly_listeners ?? entity?.artist?.monthly_listeners;
  const coverPreview = playlist && coverPreviews[playlist.id];
  const heroImages = coverPreview
    ? [{ url: coverPreview }]
    : entity?.images.length
      ? entity.images
      : playlist?.images;
  // Liked Songs, or one artist's songs in it ("Liked Songs by Eminem").
  const likedArtist = state.route.startsWith("liked-artist:")
    ? state.route.slice(13)
    : "";
  const likedRoute = state.route === "liked" || !!likedArtist;
  const localRoute = state.route === "local";
  const artistId = artistPage ? state.route.slice(7) : "";
  const [likedTotal, setLikedTotal] = useState<number>();
  useEffect(() => {
    if (state.route !== "liked" || !state.profile) return;
    let live = true;
    rpc<number>("liked_songs_count").then(
      (count) => live && setLikedTotal(count),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [state.route, state.profile]);
  // The exact length comes from the whole list at once, so big playlists
  // never show a figure that grows as their pages load.
  const durationKey =
    state.route === "liked" || state.route.startsWith("playlist:")
      ? state.route
      : "";
  const [listDuration, setListDuration] = useState<{
    key: string;
    count: number;
    ms: number;
  }>();
  useEffect(() => {
    if (!durationKey || !state.profile) return;
    let live = true;
    const known = listDurations.get(durationKey);
    if (known)
      setListDuration({ key: durationKey, count: known[0], ms: known[1] });
    rpc<[number, number]>("list_duration", { key: durationKey }).then(
      (result) => {
        listDurations.set(durationKey, result);
        if (live)
          setListDuration({
            key: durationKey,
            count: result[0],
            ms: result[1],
          });
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [durationKey, state.profile]);
  const [artistLiked, setArtistLiked] = useState<{
    id: string;
    count: number;
  } | null>(null);
  const likedArtistNames = useRef(new Map<string, string>());
  // Count again when a song on the page is liked or unliked.
  const savedHere = (entity?.overview?.popular ?? [])
    .map((item) => (state.saved[item.track.uri] ? "1" : "0"))
    .join("");
  useEffect(() => {
    if (!artistId || !state.profile) return;
    let live = true;
    rpc<number>("artist_liked_count", { id: artistId }).then(
      (count) => live && setArtistLiked({ id: artistId, count }),
      () => live && setArtistLiked({ id: artistId, count: 0 }),
    );
    return () => {
      live = false;
    };
  }, [artistId, savedHere, state.profile]);
  const likedHere = artistLiked?.id === artistId ? artistLiked.count : 0;
  const openLikedByArtist = (id: string, name: string) => {
    likedArtistNames.current.set(id, name);
    navigate(`liked-artist:${id}`);
  };
  const likedArtistName =
    likedArtist &&
    (likedArtistNames.current.get(likedArtist) ||
      state.tracks
        .flatMap((track) => track.artists)
        .find((artist) => artist.id === likedArtist)?.name);
  const artworkColor = useArtworkColor(
    likedRoute || localRoute ? undefined : imageOf(heroImages),
  );
  const heroColor = likedRoute
    ? "rgb(80 56 160)"
    : localRoute
      ? "rgb(36 56 110)"
      : (artworkColor ?? "rgb(83 83 83)");
  const playlistInfo = entity?.playlist;
  const personalizedMix = playlistInfo?.format.startsWith("daily-mix");
  const listTotal =
    playlistInfo?.total_tracks ??
    (state.route === "liked" ? likedTotal : undefined) ??
    (state.tracks.length > 0 && !state.more ? state.tracks.length : undefined);
  const exactList =
    listDuration?.key === state.route ? listDuration : undefined;
  // Without the exact figure, a fully loaded list can still be summed.
  const timed = state.tracks.filter((track) => track.duration_ms > 0);
  const playlistDuration =
    exactList && exactList.ms > 0
      ? exactList.ms
      : listTotal &&
          timed.length >= listTotal &&
          (playlistInfo || state.route === "liked" || localRoute)
        ? timed.reduce((sum, track) => sum + track.duration_ms, 0)
        : undefined;
  const heroSongCount = entity?.album?.total_tracks ?? listTotal;
  const heroDuration = entity?.album_details?.complete_duration
    ? entity.album_details.duration_ms
    : playlistDuration;
  const title =
    entity?.title ||
    playlist?.name ||
    (state.route === "liked"
      ? "Liked Songs"
      : localRoute
        ? "Local Files"
        : likedArtist
          ? likedArtistName
            ? `Liked Songs by ${likedArtistName}`
            : "Liked Songs"
          : state.route === "queue"
            ? "Queue"
            : state.route === "recent"
              ? "Recently played"
              : state.route.startsWith("search:")
                ? `Results for “${state.route.slice(7)}”`
                : state.route.startsWith("playlist:")
                  ? "Playlist"
                  : state.route.startsWith("artist:")
                    ? state.playback.item?.artists.find(
                        (artist) => artist.id === state.route.slice(7),
                      )?.name || "Artist"
                    : state.route.startsWith("album:")
                      ? "Album"
                      : "Good to see you");
  const profileName = state.profile?.display_name || state.profile?.id || "You";
  const account = state.profile?.id ?? "";
  useEffect(() => setRecentSearches(loadRecentSearches(account)), [account]);
  const updateRecentSearches = (
    change: (list: RecentSearch[]) => RecentSearch[],
  ) =>
    setRecentSearches((list) => {
      const next = change(list);
      saveRecentSearches(account, next);
      return next;
    });
  // `lyssna://` links open a page, as Spotify's own do, and never start
  // playback. A song opens its album with the song selected. One that
  // opened Lyssna waits until it is signed in and ready.
  useEffect(() => {
    if (!state.profile || !state.native_ready) return;
    rpc<{ kind: "open"; value: string; select?: string } | null>("take_link")
      .then((link) => {
        if (!link) return;
        if (link.value.startsWith("user:")) {
          const user = link.value.slice(5);
          profileTrail.current = [];
          setProfileView({ user, hint: { id: user } });
          setCenter("profile");
        } else {
          setCenter("browse");
          navigateRef.current(link.value);
          // Opening a page clears the selection, so the song is picked
          // once its album has listed it.
          linkedSong.current = link.select ?? null;
        }
      })
      .catch((error) => notify(String(error), true));
  }, [state.profile?.id, state.native_ready, linkSignal]);
  const openRecentSearch = (item: RecentSearch) => {
    setSearchFocused(false);
    searchInput.current?.blur();
    if (item.track)
      play({ command: "play", uri: item.track.uri, track: item.track });
    else if (item.route.startsWith("user:")) {
      const user = item.route.slice(5);
      profileTrail.current = [];
      setProfileView({
        user,
        hint: { id: user, display_name: item.name, images: item.images },
      });
      setCenter("profile");
    } else navigate(item.route);
  };
  // Native playlist info gives the owner's user ID; show their display name.
  const ownerId = playlistInfo?.owner || playlist?.owner.id;
  const creatorProfile = playlistInfo?.owner_profile;
  const creator =
    creatorProfile?.id && creatorProfile.id === ownerId
      ? creatorProfile
      : undefined;
  const ownerName =
    likedRoute || localRoute || (ownerId && ownerId === state.profile?.id)
      ? profileName
      : ownerId === "spotify"
        ? "Spotify"
        : creator?.display_name ||
          (playlist?.owner.id === ownerId && playlist?.owner.display_name) ||
          state.playlists.find(
            (item) => item.owner.id === ownerId && item.owner.display_name,
          )?.owner.display_name ||
          ownerId ||
          "Your collection";
  const profileImage = useLoadedImage(imageOf(state.profile?.images));
  const ownerImage = useLoadedImage(
    imageOf(
      likedRoute || localRoute || ownerId === state.profile?.id
        ? state.profile?.images
        : creator?.images,
    ),
  );
  const collection =
    likedRoute || localRoute || state.route.startsWith("playlist:");
  const collectionPlays =
    collection &&
    !likedRoute &&
    (playlist
      ? playlist.owner.id === "spotify"
      : entity?.playlist?.owner === "Spotify");
  const filtering = !!(
    songQuery.trim() ||
    songGenre ||
    songAge ||
    songSort !== "order"
  );

  const ownPlaylist = state.playlists.find(
    (item) =>
      `playlist:${item.id}` === state.route &&
      item.owner.id === state.profile?.id,
  );
  // Rows drag into place only in the playlist's own order, unfiltered.
  const reorderable =
    !filtering &&
    state.playlists.some(
      (item) =>
        `playlist:${item.id}` === state.route &&
        (item.owner.id === state.profile?.id || item.collaborative),
    );
  const displayTracks = entity?.artist
    ? (entity.overview?.popular.map((item) => item.track) ?? [])
    : collection
      ? collectionTracks(state.tracks, state.facts, {
          query: songQuery,
          genre: songGenre,
          age: songAge,
          sort: songSort,
          seed: shuffleSeed,
        })
      : state.tracks;
  const playTrack = useCallback(
    (track: Track, index?: number) => {
      if (!playable(track)) return;
      const current = stateRef.current;
      const complete = current.browse_info?.album
        ? { ...track, album: current.browse_info.album }
        : track;
      if (
        current.route.startsWith("album:") ||
        (current.route.startsWith("playlist:") && !filtering)
      )
        play({
          command: "play_context",
          context: `spotify:${current.route}`,
          uri: track.uri,
          track: complete,
        });
      else {
        const queue =
          collection || index !== undefined
            ? collectionQueue(displayTracks, track, index)
            : undefined;
        if (queue)
          play({
            command: "play_uris",
            ...queue,
            ...(likedRoute
              ? { context: "liked" }
              : current.route.startsWith("playlist:")
                ? { context: `spotify:${current.route}` }
                : {}),
            track: complete,
          });
        else play({ command: "play", uri: track.uri, track: complete });
      }
    },
    [play, collection, likedRoute, displayTracks, filtering],
  );
  // Title, Album and Date added headers sort like Spotify's on playlists
  // and Liked Songs: ascending, descending, then the playlist's own order.
  const headerSort = (column: "title" | "album" | "added", label: string) => {
    if (!collection) return label;
    const [ascending, descending] =
      column === "added"
        ? ["added_oldest", "recent"]
        : [`${column}_asc`, `${column}_desc`];
    const direction =
      songSort === ascending ? "up" : songSort === descending ? "down" : null;
    return (
      <button
        className={`column-sort ${direction ? "active" : ""}`}
        title={`Sort by ${label}`}
        aria-label={`Sort by ${label}${direction === "up" ? ", ascending" : direction === "down" ? ", descending" : ""}`}
        onClick={() => setSongSort(nextHeaderSort(column, songSort))}
      >
        {label}
        {direction === "up" && <ChevronUp size={14} />}
        {direction === "down" && <ChevronDown size={14} />}
      </button>
    );
  };
  const songGenres = [
    ...new Set(state.tracks.flatMap((track) => genresFor(track, state.facts))),
  ].sort();
  const librarySort = state.settings.library_sort ?? "recents";
  const libraryView = state.settings.library_view ?? "list";
  const visibleLibrary = libraryItems(state)
    .filter((playlist) =>
      playlist.name.toLowerCase().includes(libraryQuery.toLowerCase()),
    )
    .filter((playlist) =>
      libraryFilter === "owned"
        ? playlist.owner.id === state.profile?.id
        : libraryFilter === "collaborative"
          ? playlist.collaborative
          : libraryFilter === "" || libraryFilter === "playlists",
    );
  const showingPlaylists = ["", "playlists", "owned", "collaborative"].includes(
    libraryFilter,
  );
  const showingCollection = ["", "albums", "artists"].includes(libraryFilter);
  // Saving or following elsewhere changes what these chips list.
  const collectionKey = Object.keys(state.saved)
    .filter((uri) => state.saved[uri] && /^spotify:(album|artist):/.test(uri))
    .sort()
    .join();
  // Loaded with every view: its play times order playlists too. A new
  // context reloads it so Spotify's play history moves what was played.
  useEffect(() => {
    if (!state.profile || !state.native_ready) return;
    let active = true;
    rpc<{ albums: Album[]; artists: Artist[]; recency: Recency }>(
      "library_items",
    )
      .then((items) => active && setSavedCollection(items))
      .catch(
        () =>
          active &&
          setSavedCollection(
            (items) => items ?? { albums: [], artists: [], recency: {} },
          ),
      );
    return () => {
      active = false;
    };
  }, [
    state.profile?.id,
    state.native_ready,
    collectionKey,
    collectionReload,
    state.playback.context,
  ]);
  // What is playing now counts as most recent, as in Spotify, before its
  // play reaches Spotify's history.
  const playingSince = useMemo(() => Date.now(), [state.playback.context]);
  const recency: Recency = { ...savedCollection?.recency };
  if (state.playback.context)
    recency[state.playback.context] = Math.max(
      recency[state.playback.context] ?? 0,
      playingSince,
    );
  const collectionRows: (
    { kind: "album"; item: Album } | { kind: "artist"; item: Artist }
  )[] = [
    ...(libraryFilter !== "artists" && showingCollection
      ? (savedCollection?.albums ?? [])
          .filter((album) =>
            `${album.name} ${(album.artists ?? []).map((artist) => artist.name).join(" ")}`
              .toLowerCase()
              .includes(libraryQuery.toLowerCase()),
          )
          .map((item) => ({ kind: "album" as const, item }))
      : []),
    ...(libraryFilter !== "albums" && showingCollection
      ? (savedCollection?.artists ?? [])
          .filter((artist) =>
            artist.name.toLowerCase().includes(libraryQuery.toLowerCase()),
          )
          .map((item) => ({ kind: "artist" as const, item }))
      : []),
  ];
  // Show the artist's sticky bar once the header has scrolled under it.
  useEffect(() => {
    setHeroGone(false);
    setAboutOpen(false);
    const hero = heroRef.current,
      root = mainScroll.current;
    if (!state.route.startsWith("artist:") || !hero || !root) return;
    const observer = new IntersectionObserver(
      ([entry]) => setHeroGone(!entry.isIntersecting),
      { root, rootMargin: "-72px 0px 0px 0px" },
    );
    observer.observe(hero);
    return () => observer.disconnect();
  }, [state.route, center]);
  // Play straight from the sidebar cover, like Spotify's Your Library.
  // Like Spotify's library, the playing row is green with a speaker, and
  // its cover button pauses or resumes instead of starting over.
  const nowPlayingRoute = playingRoute(
    state.playback.context,
    !!state.dj_context,
  );
  const playingMark = (route: string) =>
    nowPlayingRoute === route &&
    state.playback.is_playing && (
      <Volume2 className="library-playing" size={16} aria-label="Playing" />
    );
  const coverPlay = (name: string, start: () => void, route: string) => {
    // Compact library rows open their playlist, as in Spotify; there is no
    // room to tell playing from opening apart.
    if (state.settings.compact_library) return null;
    const active = nowPlayingRoute === route,
      pausing = active && state.playback.is_playing;
    return (
      <button
        className="library-cover-play"
        aria-label={`${pausing ? "Pause" : "Play"} ${name}`}
        title={`${pausing ? "Pause" : "Play"} ${name}`}
        onClick={(event) => {
          event.stopPropagation();
          if (active) play({ command: pausing ? "pause" : "play" });
          else start();
        }}
      >
        {pausing ? (
          <PauseIcon size={18} />
        ) : (
          <Play size={20} fill="currentColor" />
        )}
      </button>
    );
  };
  const playLiked = () => {
    const first = displayTracks.find(playable);
    if (stateRef.current.route === "liked" && first)
      playTrack(first, displayTracks.indexOf(first));
    else {
      likedPlay.current = true;
      navigate("liked");
    }
  };
  useEffect(() => {
    if (!likedPlay.current) return;
    // Leaving Liked Songs before its songs load cancels the start.
    if (state.route !== "liked") {
      likedPlay.current = false;
      return;
    }
    const first = displayTracks.find(playable);
    if (!first) return;
    likedPlay.current = false;
    playTrack(first, displayTracks.indexOf(first));
  }, [state.route, displayTracks, playTrack]);
  const playlistRow = (playlist: Playlist) => (
    <div
      className={`library-row ${nowPlayingRoute === `playlist:${playlist.id}` ? "playing" : ""}`}
      key={playlist.id}
      data-playlist-id={playlist.id}
      data-context-route={`playlist:${playlist.id}`}
      data-context-name={playlist.name}
    >
      <button
        className={`library-item ${state.route === `playlist:${playlist.id}` ? "current" : ""}`}
        onClick={() =>
          playlist.id === DJ_ID
            ? play({ command: "dj" })
            : navigate(`playlist:${playlist.id}`)
        }
        {...(playlist.id === DJ_ID
          ? {}
          : prefetchOn(`playlist:${playlist.id}`))}
      >
        {playlist.id === DJ_ID ? (
          <DJArtwork />
        ) : (
          <Cover images={playlist.images} />
        )}
        <div>
          <strong>{playlist.name}</strong>
          <small>
            {accountOrder(state).pinned.includes(playlist.id) && (
              <Pin size={12} className="pinned-mark" />
            )}
            {playlist.id === DJ_ID
              ? "Click to start listening"
              : playlist.owner.display_name
                ? `Playlist · ${playlist.owner.display_name}`
                : // An unresolved owner is only a raw user ID.
                  "Playlist"}
          </small>
        </div>
      </button>
      <button
        className={`library-pin icon-button ${accountOrder(state).pinned.includes(playlist.id) ? "pinned" : ""}`}
        title={`${accountOrder(state).pinned.includes(playlist.id) ? "Unpin" : "Pin"} ${playlist.name}`}
        aria-pressed={accountOrder(state).pinned.includes(playlist.id)}
        onClick={() => togglePin(playlist)}
      >
        <Pin size={14} />
      </button>
      {playingMark(`playlist:${playlist.id}`)}
      {playlist.id !== DJ_ID &&
        coverPlay(
          playlist.name,
          () =>
            play({
              command: "play_home",
              context: `spotify:playlist:${playlist.id}`,
            }),
          `playlist:${playlist.id}`,
        )}
    </div>
  );
  // Searching or filtering lists matching playlists flat, without folders.
  const libraryEntries: LibraryEntry[] =
    libraryQuery || (libraryFilter !== "" && libraryFilter !== "playlists")
      ? visibleLibrary.map((playlist) => ({ kind: "playlist", playlist }))
      : libraryTree(
          librarySort === "custom"
            ? customOrder(
                visibleLibrary,
                state.playlists,
                accountOrder(state).pinned,
              )
            : visibleLibrary,
          state.playlists,
          accountOrder(state).pinned,
          state.library_folders,
        );
  type FolderEntry = Extract<LibraryEntry, { kind: "folder" }>;
  let openFolder: FolderEntry | undefined;
  if (!libraryQuery && (libraryFilter === "" || libraryFilter === "playlists"))
    for (const id of folderPath) {
      const next = (openFolder?.entries ?? libraryEntries).find(
        (entry): entry is FolderEntry =>
          entry.kind === "folder" && entry.folder.id === id,
      );
      if (!next) break;
      openFolder = next;
    }
  const collectionEntry = ({
    kind,
    item,
  }: (typeof collectionRows)[number]): ReactNode => {
    const route = `${kind}:${item.id}`;
    return (
      <button
        key={item.uri}
        className={`library-item ${state.route === route ? "current" : ""}`}
        data-context-route={route}
        data-context-name={item.name}
        onClick={() => navigate(route)}
        {...prefetchOn(route)}
      >
        <Cover
          images={item.images}
          className={kind === "artist" ? "artist-cover" : ""}
        />
        <div>
          <strong>{item.name}</strong>
          <small>
            {kind === "album"
              ? `Album · ${((item as Album).artists ?? []).map((artist) => artist.name).join(", ")}`
              : "Artist"}
          </small>
        </div>
      </button>
    );
  };
  // Playlists, folders, albums and artists in one list, ordered like
  // Spotify's Recents with pinned playlists on top.
  const pinnedIds = accountOrder(state).pinned;
  type LibraryRow =
    | { entry: LibraryEntry; row?: undefined }
    | { entry?: undefined; row: (typeof collectionRows)[number] };
  const entryFacts = (entry: LibraryEntry): LibraryFacts =>
    entry.kind === "playlist"
      ? {
          recent: entryRecency(entry, recency),
          added: entryAdded(entry),
          name: entry.playlist.name,
          creator: entry.playlist.owner.display_name ?? entry.playlist.owner.id,
        }
      : {
          recent: entryRecency(entry, recency),
          added: entryAdded(entry),
          name: entry.folder.name,
          creator: "",
        };
  const libraryRows = sortLibrary<LibraryRow>(
    [
      ...libraryEntries.map((entry) => ({ entry })),
      ...collectionRows.map((row) => ({ row })),
    ],
    librarySort,
    (item) => {
      if (item.entry) return entryFacts(item.entry);
      const { kind, item: saved } = item.row;
      const time = recency[saved.uri ?? ""] ?? 0;
      return {
        recent: time,
        added: time,
        name: saved.name,
        creator:
          kind === "album"
            ? ((saved as Album).artists ?? [])
                .map((artist) => artist.name)
                .join(", ")
            : saved.name,
      };
    },
    ({ entry }) =>
      entry?.kind === "playlist" && pinnedIds.includes(entry.playlist.id),
  );
  // Like Spotify, a folder opens as its own list of playlists.
  const libraryEntry = (entry: LibraryEntry): ReactNode =>
    entry.kind === "playlist" ? (
      playlistRow(entry.playlist)
    ) : (
      <button
        className="library-item spotify-folder"
        key={entry.folder.id}
        onClick={() => setFolderPath((path) => [...path, entry.folder.id])}
        onContextMenu={(event) => {
          event.preventDefault();
          setSongMenu(null);
          setEntityMenu(null);
          setFolderMenu({
            folder: entry.folder,
            x: event.clientX,
            y: event.clientY,
          });
        }}
      >
        <div className="folder-cover">
          <Folder size={22} />
        </div>
        <div>
          <strong>{entry.folder.name}</strong>
          <small>
            Folder · {entry.count} playlist{entry.count === 1 ? "" : "s"}
          </small>
        </div>
      </button>
    );
  const loadMore = useCallback(() => {
    const current = stateRef.current;
    const key = `${current.generation}:${current.tracks.length}`;
    if (current.loading || !current.more || pendingPage.current === key) return;
    pendingPage.current = key;
    dispatch({ type: "page_loading", generation: current.generation });
    void rpc("browse", {
      key: current.route,
      generation: current.generation,
      offset: current.tracks.length,
    }).catch((error) =>
      dispatch({
        type: "tracks_failed",
        generation: current.generation,
        message: String(error),
      }),
    );
  }, []);
  useEffect(() => {
    if (!state.loading) pendingPage.current = "";
  }, [state.loading, state.tracks.length, state.generation]);
  // Playlists, Liked Songs and albums load every page at once, like Spotify.
  const loadAll = collection || state.route.startsWith("album:");
  useEffect(() => {
    if (loadAll && state.more && !state.loading) loadMore();
  }, [loadAll, state.more, state.loading, state.tracks.length, loadMore]);
  useEffect(() => {
    const sentinel = pageSentinel.current,
      root = mainScroll.current;
    if (center !== "browse" || !sentinel || !root || !state.more) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) loadMore();
      },
      { root, rootMargin: "1200px 0px" },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [
    center,
    state.route,
    state.more,
    state.loading,
    state.tracks.length,
    loadMore,
  ]);
  const resizeRight = (event: React.PointerEvent) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    let width: number | null = null;
    const move = (event: PointerEvent) => {
      width = Math.round(
        Math.max(300, Math.min(520, window.innerWidth - event.clientX - 8)),
      );
      setRightDrag(width);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (width !== null) updateSettings({ now_playing_width: width });
      setRightDrag(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  // Like Spotify: drag the library's edge to resize it; drag far enough
  // left to collapse it to covers, and back out to expand it again.
  const resizeLeft = (event: React.PointerEvent) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    const left =
      event.currentTarget.parentElement?.getBoundingClientRect().left ?? 0;
    let kept: number | null = null;
    const move = (event: PointerEvent) => {
      const width = event.clientX - left - 8;
      const compact = width < 170;
      if (compact !== !!stateRef.current.settings.compact_library)
        updateSettings({ compact_library: compact });
      if (!compact) {
        kept = Math.round(Math.max(250, Math.min(480, width)));
        setLeftDrag(kept);
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (kept !== null) updateSettings({ library_width: kept });
      setLeftDrag(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  const showTrackList =
    state.route !== "home" && (!entity?.artist || displayTracks.length > 0);
  const listedTracks =
    entity?.artist && !popularMore ? displayTracks.slice(0, 5) : displayTracks;
  const rows = useRowWindow(
    mainScroll,
    trackTable,
    listedTracks.length,
    `${center}:${state.route}`,
  );
  const selectRow = (
    index: number,
    keys: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean },
  ) => {
    const uri = listedTracks[index]?.uri;
    if (!uri) return;
    const add = keys.ctrlKey || keys.metaKey,
      anchor = selectAnchor.current;
    if (keys.shiftKey && anchor !== null) {
      const range = listedTracks
        .slice(Math.min(anchor, index), Math.max(anchor, index) + 1)
        .map((track) => track.uri);
      setSelected((old) => new Set([...(add ? old : []), ...range]));
      return;
    }
    selectAnchor.current = index;
    setSelected((old) => {
      if (!add) return new Set([uri]);
      const next = new Set(old);
      if (next.has(uri)) next.delete(uri);
      else next.add(uri);
      return next;
    });
  };
  const listedRef = useRef(listedTracks);
  listedRef.current = listedTracks;
  useEffect(() => {
    const uri = linkedSong.current;
    const index = uri
      ? listedTracks.findIndex((track) => track.uri === uri)
      : -1;
    if (!uri || index < 0) return;
    linkedSong.current = null;
    setSelected(new Set([uri]));
    selectAnchor.current = index;
    requestAnimationFrame(() =>
      mainScroll.current
        ?.querySelector<HTMLElement>(
          `.track-row[data-uri="${CSS.escape(uri)}"]`,
        )
        ?.scrollIntoView({ block: "center" }),
    );
  }, [listedTracks]);
  const anySelected = selected.size > 0;
  useEffect(() => {
    if (!anySelected) return;
    const typing = (target: EventTarget | null) =>
      target instanceof HTMLElement &&
      !!target.closest("input, textarea, select, [contenteditable=true]");
    const key = (event: KeyboardEvent) => {
      if (typing(event.target)) return;
      if (event.key === "Escape") setSelected(new Set());
      else if (
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === "a"
      ) {
        event.preventDefault();
        setSelected(new Set(listedRef.current.map((track) => track.uri)));
      }
    };
    // Clicking anywhere but a song or a menu clears the selection.
    const outside = (event: PointerEvent) => {
      if (
        !(event.target as HTMLElement).closest?.(
          ".track-row, .popover, .popover-dismiss, [role=menu]",
        )
      )
        setSelected(new Set());
    };
    document.addEventListener("keydown", key);
    document.addEventListener("pointerdown", outside);
    return () => {
      document.removeEventListener("keydown", key);
      document.removeEventListener("pointerdown", outside);
    };
  }, [anySelected]);
  return (
    <div
      className={`application ${IS_MAC ? "macos" : ""} ${state.settings.reduce_motion ? "reduce-motion" : ""} ${immersive ? "immersive" : ""} ${state.settings.liquid_glass ? "liquid-glass" : ""} ${state.settings.liquid_glass ? "glass-refraction" : ""}`}
      style={fonts}
      onDragOver={(event) => event.preventDefault()}
      onContextMenu={(event) => {
        if (event.defaultPrevented) return;
        setSongMenu(null);
        setEntityMenu(null);
        const target = event.target as HTMLElement;
        if (target.closest('input,textarea,[contenteditable="true"]')) {
          setSongMenu(null);
          setEntityMenu(null);
          return;
        }
        event.preventDefault();
        if (!stateRef.current.profile) return;
        const element = target.closest<HTMLElement>("[data-context-route]");
        const route = element?.dataset.contextRoute;
        if (!element || !route) {
          setSongMenu(null);
          setEntityMenu(null);
          return;
        }
        const match = /^(playlist|album|artist|track):([A-Za-z0-9]{22})$/.exec(
          route,
        );
        if (match?.[1] === "track") {
          const current = stateRef.current,
            track = [
              ...current.tracks,
              ...current.details.queue,
              ...(current.playback.item ? [current.playback.item] : []),
            ].find(
              (item) =>
                item.id === match[2] ||
                item.uri === `spotify:track:${match[2]}`,
            );
          if (track) {
            setEntityMenu(null);
            setSongMenu({
              track,
              queued: element.dataset.queued === "true",
              x: event.clientX,
              y: event.clientY,
            });
          }
          return;
        }
        if (match || route === "liked") {
          setSongMenu(null);
          setEntityMenu({
            kind:
              route === "liked" ? "liked" : (match![1] as EntityMenu["kind"]),
            id: match?.[2] ?? "",
            name:
              element.dataset.contextName || element.textContent?.trim() || "",
            x: event.clientX,
            y: event.clientY,
          });
        }
      }}
      onDrop={(event) => {
        event.preventDefault();
        const file = event.dataTransfer.files[0];
        if (file) void importFile(file);
      }}
    >
      {state.settings.liquid_glass && <GlassRefractionFilter />}
      {state.settings.liquid_glass && (
        <GlassBackdrop
          track={state.playback.item}
          playing={state.playback.is_playing}
          reduceMotion={state.settings.reduce_motion}
        />
      )}
      <header
        className={`topbar ${sessionScreen(state) === "client" ? "" : "signed-out"}`}
        // Empty top-bar space moves the window, but only once the pointer
        // actually moves: starting a drag on press made Windows restore a
        // maximized window on a plain click.
        onPointerDown={(event) => {
          if (event.button === 0 && event.target === event.currentTarget)
            dragStart.current = { x: event.clientX, y: event.clientY };
        }}
        onPointerMove={(event) => {
          const start = dragStart.current;
          if (
            start &&
            Math.hypot(event.clientX - start.x, event.clientY - start.y) > 4
          ) {
            dragStart.current = null;
            void windowHandle().startDragging();
          }
        }}
        onPointerUp={() => {
          dragStart.current = null;
        }}
        onDoubleClick={(event) => {
          if (event.target === event.currentTarget)
            void windowHandle().toggleMaximize();
        }}
      >
        {!nativeMenuReady && <AppMenu groups={menuGroups} />}
        <div className="history-buttons">
          <button className="icon-button" title="Go back" onClick={back}>
            <ArrowLeft size={19} />
          </button>
          <button className="icon-button" title="Go forward" onClick={forward}>
            <ArrowRight size={19} />
          </button>
        </div>
        <div className="search-group">
          <button
            className={`icon-button home-button ${state.route === "home" && center === "browse" ? "active" : ""}`}
            title="Home"
            onClick={() => navigate("home")}
          >
            {state.route === "home" && center === "browse" ? (
              // Filled while Home is open, as Spotify's is.
              <svg
                width={23}
                height={23}
                viewBox="0 0 24 24"
                aria-hidden="true"
                fill="currentColor"
                stroke="currentColor"
                strokeWidth={2}
                strokeLinejoin="round"
              >
                <path d="M12 3 3.5 9.8V20h5.5v-6h6v6h5.5V9.8z" />
              </svg>
            ) : (
              <Home size={23} />
            )}
          </button>
          <div className="search-box">
            <Search size={21} />
            <input
              ref={searchInput}
              aria-label="Search Spotify"
              placeholder="What do you want to play?"
              value={query}
              disabled={!state.profile}
              onChange={(event) => setQuery(event.target.value)}
              onFocus={() => setSearchFocused(true)}
              onBlur={() => setSearchFocused(false)}
              onKeyDown={(event) => {
                if (event.key === "Escape") event.currentTarget.blur();
                if (event.key === "Enter" && query.trim()) {
                  event.preventDefault();
                  navigate(`search:${query.trim()}`);
                }
              }}
            />
            {query && (
              <button
                className="icon-button"
                aria-label="Clear search"
                onClick={() => {
                  setQuery("");
                  if (state.route.startsWith("search:")) navigate("home");
                }}
              >
                <X size={17} />
              </button>
            )}
            {searchFocused && !query && recentSearches.length > 0 && (
              <div
                className="recent-searches popover"
                role="listbox"
                aria-label="Recent searches"
                // Keep focus in the box so clicks land before it closes.
                onMouseDown={(event) => event.preventDefault()}
              >
                <h3>Recent searches</h3>
                {recentSearches.map((item) => (
                  <div className="recent-search" key={item.uri}>
                    <button
                      role="option"
                      aria-selected={false}
                      onClick={() => openRecentSearch(item)}
                    >
                      <Cover
                        images={item.images}
                        className={
                          item.kind === "Artist" || item.kind === "Profile"
                            ? "round"
                            : ""
                        }
                      />
                      <span>
                        <strong>{item.name}</strong>
                        <small>
                          {item.kind}
                          {item.subtitle &&
                          item.kind !== "Artist" &&
                          item.kind !== "Profile"
                            ? ` • ${item.subtitle}`
                            : ""}
                        </small>
                      </span>
                    </button>
                    <button
                      className="icon-button"
                      aria-label={`Remove ${item.name} from recent searches`}
                      onClick={() =>
                        updateRecentSearches((list) =>
                          list.filter((entry) => entry.uri !== item.uri),
                        )
                      }
                    >
                      <X size={16} />
                    </button>
                  </div>
                ))}
                <button
                  className="recent-searches-clear"
                  onClick={() => updateRecentSearches(() => [])}
                >
                  Clear recent searches
                </button>
              </div>
            )}
          </div>
        </div>
        <div className="top-actions">
          {((nightlyCommit && state.settings.update_channel === "nightly") ||
            switchingToNightly) && (
            <NightlyButton
              status={state.update_status}
              report={state.nightly}
              checking={checkingNightly}
              automatic={state.settings.auto_download_updates}
              onCheck={() => void checkNightly()}
              onShow={() =>
                switchingToNightly
                  ? setFirstBuildOpen(true)
                  : window.dispatchEvent(new Event("lyssna:show-update"))
              }
              onRestart={() => void call("app_update", { action: "install" })}
            />
          )}
          <button
            className={`icon-button whats-new-button ${state.route === "whats-new" && center === "browse" ? "active" : ""}`}
            title="What's New"
            aria-label="What's New"
            onClick={() => navigate("whats-new")}
          >
            <Bell size={20} />
          </button>
          <ProfileMenu
            name={profileName}
            image={profileImage}
            onAction={(action) => {
              if (action === "account")
                void call("open_link", {
                  url: "https://www.spotify.com/account/overview/",
                });
              else if (action === "profile" && state.profile) {
                profileTrail.current = [];
                setProfileView({ user: state.profile.id, hint: state.profile });
                setCenter("profile");
              } else if (action === "recents") navigate("recent");
              else if (action === "settings")
                setSettingsTab(
                  lastSettingsTab(
                    state.settings.settings_remember_place !== false,
                  ),
                );
              else void call("sign_out");
            }}
          />
        </div>
        {!IS_MAC && (
          <div className="window-controls">
            <button
              aria-label="Minimize window"
              onClick={() => void windowHandle().minimize()}
            >
              <Minus size={15} />
            </button>
            <button
              aria-label="Maximize or restore window"
              onClick={() => void windowHandle().toggleMaximize()}
            >
              <Square size={13} />
            </button>
            <button
              className="window-close"
              aria-label="Close window"
              onClick={() => void windowHandle().close()}
            >
              <X size={18} />
            </button>
          </div>
        )}
      </header>
      <div
        className="workspace"
        style={
          {
            "--right-width": `${sidebar ? rightWidth : 0}px`,
            ...(leftWidth ? { "--left-width": `${leftWidth}px` } : {}),
          } as CSSProperties
        }
        inert={!state.profile}
      >
        <aside
          className={`library-panel ${state.settings.compact_library ? "compact-library" : ""}`}
        >
          <div className="library-heading">
            <button
              className="text-link"
              onClick={() =>
                updateSettings({
                  compact_library: !state.settings.compact_library,
                })
              }
            >
              <Library size={20} />
              <strong>Your Library</strong>
            </button>
            <div>
              <button
                className="icon-button"
                title="Create folder"
                onClick={() => setFolderDialog(true)}
              >
                <FolderPlus size={18} />
              </button>
            </div>
          </div>
          <div className="library-filters">
            {[
              ["playlists", "Playlists"],
              ["owned", "By you"],
              ["collaborative", "Collaborative"],
              ["albums", "Albums"],
              ["artists", "Artists"],
            ].map(([key, label]) => (
              <button
                key={key}
                className={`chip ${libraryFilter === key ? "selected" : ""}`}
                aria-pressed={libraryFilter === key}
                // Clicking the chosen chip again shows everything.
                onClick={() =>
                  setLibraryFilter((current) => (current === key ? "" : key))
                }
              >
                {label}
              </button>
            ))}
          </div>
          <div className="library-tools">
            <label className="library-search">
              <Search size={16} />
              <input
                placeholder="Search in your library"
                aria-label="Search library"
                value={libraryQuery}
                onChange={(event) => setLibraryQuery(event.target.value)}
              />
            </label>
            <LibrarySortMenu
              sort={librarySort}
              view={libraryView}
              onSort={(library_sort) => updateSettings({ library_sort })}
              onView={(library_view) => updateSettings({ library_view })}
            />
          </div>
          <div className={`library-list library-view-${libraryView}`}>
            {openFolder && (
              <div className="library-folder-header">
                <button
                  className="icon-button"
                  title="Back"
                  aria-label="Back"
                  onClick={() => setFolderPath((path) => path.slice(0, -1))}
                >
                  <ChevronLeft size={20} />
                </button>
                <strong>{openFolder.folder.name}</strong>
              </div>
            )}
            <div
              className={`library-liked ${nowPlayingRoute === "liked" ? "playing" : ""}`}
              hidden={!!openFolder || !showingPlaylists}
            >
              <button
                className={`library-item ${state.route === "liked" ? "current" : ""}`}
                data-context-route="liked"
                data-context-name="Liked Songs"
                onClick={() => navigate("liked")}
                {...prefetchOn("liked")}
              >
                <div className="liked-cover">
                  <Heart size={22} fill="currentColor" />
                </div>
                <div>
                  <strong>Liked Songs</strong>
                  <small>Playlist · Pinned</small>
                </div>
              </button>
              {playingMark("liked")}
              {coverPlay("Liked Songs", playLiked, "liked")}
            </div>
            {state.settings.show_local_files && (
              <div
                className="library-local"
                hidden={!!openFolder || !showingPlaylists}
              >
                <button
                  className={`library-item ${localRoute ? "current" : ""}`}
                  data-context-route="local"
                  data-context-name="Local Files"
                  onClick={() => navigate("local")}
                >
                  <div className="liked-cover local-cover">
                    <Folder size={22} fill="currentColor" />
                  </div>
                  <div>
                    <strong>Local Files</strong>
                    <small>
                      Playlist · {localCount}{" "}
                      {localCount === 1 ? "track" : "tracks"}
                    </small>
                  </div>
                </button>
              </div>
            )}
            {openFolder ? (
              <>
                {sortLibrary(openFolder.entries, librarySort, entryFacts).map(
                  libraryEntry,
                )}
                {!openFolder.entries.length && (
                  <p className="empty-library">This folder is empty.</p>
                )}
              </>
            ) : (
              libraryRows.map((item) =>
                item.entry
                  ? libraryEntry(item.entry)
                  : collectionEntry(item.row),
              )
            )}
            {!openFolder &&
              showingPlaylists &&
              state.settings.folders.map((folder, index) => (
                <details
                  className="library-folder"
                  key={`${folder.name}-${index}`}
                >
                  <summary>
                    {folder.name}
                    <span>{folder.playlist_ids.length}</span>
                  </summary>
                  {state.playlists
                    .filter((playlist) =>
                      folder.playlist_ids.includes(playlist.id),
                    )
                    .map((playlist) => (
                      <button
                        className="library-item"
                        key={playlist.id}
                        data-context-route={`playlist:${playlist.id}`}
                        data-context-name={playlist.name}
                        onClick={() => navigate(`playlist:${playlist.id}`)}
                        {...prefetchOn(`playlist:${playlist.id}`)}
                      >
                        <Cover images={playlist.images} />
                        <div>
                          <strong>{playlist.name}</strong>
                          <small>Playlist</small>
                        </div>
                      </button>
                    ))}
                </details>
              ))}
            {(libraryFilter === "albums" || libraryFilter === "artists") &&
              savedCollection &&
              !collectionRows.length && (
                <p className="empty-library">
                  {libraryQuery
                    ? "Nothing matches."
                    : libraryFilter === "albums"
                      ? "Albums you save appear here."
                      : "Artists you follow appear here."}
                </p>
              )}
            {(libraryFilter === "albums" || libraryFilter === "artists") &&
              !savedCollection && <p className="empty-library">Loading…</p>}
            {showingPlaylists &&
              !visibleLibrary.length &&
              !collectionRows.length &&
              state.playlists.length > 0 && (
                <p className="empty-library">
                  {libraryFilter === ""
                    ? "Nothing matches."
                    : "No playlists match."}
                </p>
              )}
          </div>
        </aside>
        <div
          className="panel-resizer library-resizer"
          title="Resize Your Library"
          onPointerDown={resizeLeft}
        />
        <main
          className={`center-panel ${center === "lyrics" ? "showing-lyrics" : ""}`}
        >
          {center === "lyrics" ? (
            <LyricsPage
              state={state}
              onCanvasActive={setCanvasInLyrics}
              immersive={immersive}
              onPlayer={play}
              onAlbum={openAlbum}
              onArtist={openArtist}
              onImport={() => setTtmlOpen("load")}
              onRetry={() => void call("retry_lyrics")}
              onSave={save}
            >
              <div
                className="lyrics-toolbar"
                role="toolbar"
                aria-label="Lyrics"
              >
                {(
                  [
                    ["romanized", "Romanization", TranslateIcon],
                    [
                      "experimental_word_sync",
                      "Experimental word sync",
                      WordSyncIcon,
                    ],
                    ["lyrics_compact", "Compact lyrics", CompactIcon],
                    ["lyrics_hide_artwork", "Hide artwork", HideArtworkIcon],
                    [
                      "lyrics_artwork_right",
                      "Artwork on the right",
                      SwapSidesIcon,
                    ],
                  ] as const
                ).map(([key, label, Icon]) => {
                  const on = !!state.settings[key];
                  return (
                    <button
                      key={key}
                      className={`icon-button ${on ? "active" : ""}`}
                      data-icon={key}
                      title={label}
                      aria-pressed={on}
                      onClick={() =>
                        // Layout toggles glide to their new place.
                        key === "lyrics_artwork_right" ||
                        key === "lyrics_hide_artwork" ||
                        key === "lyrics_compact"
                          ? animateLayout(
                              () => updateSettings({ [key]: !on }),
                              "lyrics",
                              state.settings.reduce_motion,
                            )
                          : updateSettings({ [key]: !on })
                      }
                    >
                      <Icon />
                    </button>
                  );
                })}
                <span className="toolbar-divider" aria-hidden="true" />
                <button
                  className="icon-button"
                  title="Load TTML"
                  data-icon="upload"
                  onClick={() => setTtmlOpen("load")}
                >
                  <UploadIcon />
                </button>
                {state.settings.lyrics_cache_button !== false && (
                  <button
                    className="icon-button"
                    title={CACHE_ACTIONS[cacheAction].title}
                    data-icon="retry"
                    onClick={() =>
                      void (
                        cacheAction === "all"
                          ? call("clear_lyrics_cache")
                          : call("refresh_lyrics", {
                              keepSaved: cacheAction === "reload",
                            })
                      ).then(() => notify("Fetching fresh lyrics…"))
                    }
                  >
                    <RetryIcon />
                  </button>
                )}
                <button
                  className="icon-button"
                  title="Full screen"
                  data-icon="fullscreen"
                  onClick={() => void fullscreen()}
                >
                  <FullScreenIcon />
                </button>
                <button
                  className="icon-button"
                  title="Lyssna settings"
                  data-icon="settings"
                  onClick={() =>
                    setSettingsTab(
                      lastSettingsTab(
                        state.settings.settings_remember_place !== false,
                      ),
                    )
                  }
                >
                  <SettingsIcon size={18} />
                </button>
              </div>
            </LyricsPage>
          ) : center === "profile" ? (
            profileView && (
              <ProfilePage
                user={profileView.user}
                hint={profileView.hint}
                ready={state.native_ready}
                onNavigate={homeNavigate}
                onPlay={homePlay}
                onUser={(user, hint) => {
                  profileTrail.current.push(profileView);
                  setProfileView({ user, hint });
                }}
                onOpen={(url) => void call("open_link", { url })}
                onNotice={notify}
              />
            )
          ) : state.route.startsWith("artist-shelf:") ? (
            <div className="browse-scroll" ref={mainScroll}>
              {(() => {
                const [, artist, index] = state.route.split(":");
                const section =
                  state.pages[`artist:${artist}`]?.info?.overview?.sections?.[
                    Number(index)
                  ];
                return section ? (
                  <div className="artist-shelf-page">
                    <h1>{section.title}</h1>
                    <Shelf
                      section={section}
                      navigate={openCard}
                      play={homePlay}
                      ready={state.native_ready}
                      defaultExpanded
                      page
                    />
                  </div>
                ) : (
                  <p className="discography-message">
                    Open the artist again to see this list.
                  </p>
                );
              })()}
            </div>
          ) : state.route.startsWith("discography:") ? (
            <div className="browse-scroll" ref={mainScroll}>
              <DiscographyPage
                artistId={state.route.slice("discography:".length)}
                artistName={
                  state.pages[
                    `artist:${state.route.slice("discography:".length)}`
                  ]?.info?.title ?? "Discography"
                }
                ready={state.native_ready}
                onOpenAlbum={openAlbum}
                onPlay={(context, uri) =>
                  play({ command: "play_home", context, uri })
                }
              />
            </div>
          ) : state.route === "whats-new" ? (
            <div className="browse-scroll" ref={mainScroll}>
              <WhatsNewPage
                account={state.profile?.id}
                ready={state.native_ready}
                saved={state.saved}
                onOpen={(route) => navigate(route)}
                onPlay={(uri) => play({ command: "play_home", context: uri })}
                onSave={(uri, saved) => {
                  dispatch({ type: "saved", uri, saved });
                  void call("save_item", { uri, saved });
                }}
              />
            </div>
          ) : state.route === "recent" ? (
            <div className="browse-scroll" ref={mainScroll}>
              <RecentsPage
                tracks={state.tracks}
                playlists={state.playlists}
                loading={state.loading}
                onOpen={(route) => navigate(route)}
                onPlay={playTrack}
              />
            </div>
          ) : state.route.startsWith("search:") ? (
            <div className="browse-scroll search-scroll" ref={mainScroll}>
              <SearchPage
                key={state.route}
                state={state}
                navigate={homeNavigate}
                play={homePlay}
                playTrack={playTrack}
                openArtist={openArtist}
                openAlbum={openAlbum}
                save={save}
                follow={follow}
                retry={() => navigate(state.route, false)}
                remember={(entry: SearchEntry) =>
                  updateRecentSearches((list) => rememberSearch(list, entry))
                }
                openUser={(user, hint) => {
                  profileTrail.current = [];
                  setProfileView({ user, hint });
                  setCenter("profile");
                }}
                addResults={(results) =>
                  dispatch({ type: "search_more", key: state.route, results })
                }
                notify={notify}
              />
            </div>
          ) : (
            <div className="browse-scroll" ref={mainScroll}>
              {state.route === "home" ? (
                <HomePage
                  data={state.home}
                  playlists={state.playlists}
                  loading={state.home_loading}
                  error={state.home_error}
                  ready={state.native_ready}
                  navigate={homeNavigate}
                  play={homePlay}
                  dj={startDJ}
                  refresh={refreshHome}
                  saved={state.saved}
                  onSave={(item) => {
                    const saved = !state.saved[item.uri];
                    dispatch({ type: "saved", uri: item.uri, saved });
                    void call("save_item", { uri: item.uri, saved });
                  }}
                />
              ) : (
                <>
                  {artistPage && (
                    <div
                      className={`artist-sticky ${heroGone ? "shown" : ""}`}
                      style={{ "--hero": heroColor } as CSSProperties}
                      aria-hidden={!heroGone}
                    >
                      <button
                        className="primary-play"
                        tabIndex={heroGone ? 0 : -1}
                        aria-label={`Play ${title}`}
                        onClick={() =>
                          play({
                            command: "play_home",
                            context: `spotify:${state.route}`,
                          })
                        }
                      >
                        <Play size={22} fill="currentColor" />
                      </button>
                      <strong>{title}</strong>
                    </div>
                  )}
                  <div
                    className={`entity-header ${artistPage ? "artist-header" : ""}`}
                    style={{ "--hero": heroColor } as CSSProperties}
                  >
                    <section
                      ref={heroRef}
                      className={`page-hero ${artistPage ? `artist-hero ${artistFallback ? "artist-fallback" : ""}` : ""}`}
                      style={
                        artistPage && !artistFallback && artistBanner
                          ? {
                              backgroundImage: `linear-gradient(0deg,#000c,#0000 65%),url("${artistBanner}")`,
                            }
                          : undefined
                      }
                    >
                      {artistFallback ? (
                        <Cover
                          images={heroImages}
                          className="hero-cover"
                          contextRoute={state.route}
                          contextName={title}
                        />
                      ) : likedRoute ? (
                        <div className="liked-cover hero-cover">
                          <Heart size={84} fill="currentColor" />
                        </div>
                      ) : localRoute ? (
                        <div className="liked-cover local-cover hero-cover">
                          <Folder size={84} fill="currentColor" />
                        </div>
                      ) : ownPlaylist && !artistPage ? (
                        // Like Spotify, your own playlist's cover opens Edit details.
                        <button
                          className="hero-cover hero-cover-edit"
                          aria-label="Choose photo"
                          onClick={() => openEditDialog(ownPlaylist)}
                        >
                          <Cover
                            images={heroImages ?? []}
                            contextRoute={state.route}
                            contextName={title}
                          />
                          <span className="hero-cover-hint" aria-hidden="true">
                            <Pencil size={40} />
                            Choose photo
                          </span>
                        </button>
                      ) : (
                        (!!entity?.images.length ||
                          !!playlist?.images.length) &&
                        !artistPage && (
                          <Cover
                            images={heroImages}
                            className="hero-cover"
                            contextRoute={state.route}
                            contextName={title}
                          />
                        )
                      )}
                      <div className="hero-copy">
                        {!artistPage && (
                          <span className="eyebrow">
                            {state.route.startsWith("album:")
                              ? albumTypeLabel(entity?.album?.album_type)
                              : state.route.startsWith("search:")
                                ? "Search"
                                : state.route === "queue"
                                  ? "Up next"
                                  : "Playlist"}
                          </span>
                        )}
                        <h1 className={titleSize(title)}>
                          <span
                            data-context-route={state.route}
                            data-context-name={title}
                            className={
                              ownPlaylist ? "editable-title" : undefined
                            }
                            title={ownPlaylist ? "Edit details" : undefined}
                            onClick={
                              ownPlaylist
                                ? () => openEditDialog(ownPlaylist)
                                : undefined
                            }
                          >
                            {title}
                          </span>
                        </h1>
                        {artistPage && artistVerified && (
                          <p className="artist-verified">
                            <VerifiedBadge />
                            <span>Verified by Spotify</span>
                          </p>
                        )}
                        {artistPage && monthlyListeners != null && (
                          <p className="artist-monthly">
                            {monthlyListeners.toLocaleString(undefined, {
                              notation: "compact",
                              maximumFractionDigits: 1,
                            })}{" "}
                            monthly listeners
                          </p>
                        )}
                        {state.route.startsWith("playlist:") &&
                          entity?.subtitle &&
                          entity.subtitle !== playlistInfo?.owner && (
                            <p className="hero-description">
                              <SpotifyText
                                text={entity.subtitle}
                                navigate={homeNavigate}
                              />
                            </p>
                          )}
                        {!artistPage && (
                          <p
                            className={`hero-meta ${entity?.album ? "" : "playlist-meta"}`}
                          >
                            {entity?.album ? (
                              <strong>
                                <ArtistLinks
                                  artists={entity.album.artists || []}
                                  onArtist={openArtist}
                                />
                              </strong>
                            ) : (
                              <>
                                {ownerImage && (
                                  <img
                                    className="hero-owner"
                                    src={ownerImage}
                                    alt=""
                                  />
                                )}
                                <strong>
                                  {personalizedMix
                                    ? `Made for ${profileName}`
                                    : ownerName}
                                </strong>
                              </>
                            )}
                            {entity?.album?.release_date && (
                              <span>
                                {entity.album.release_date.slice(0, 4)}
                              </span>
                            )}
                            {heroSongCount != null && (
                              <span>
                                {heroSongCount} songs
                                {heroDuration != null &&
                                  `, ${longDuration(heroDuration)}`}
                              </span>
                            )}
                          </p>
                        )}
                      </div>
                    </section>
                    {!state.route.startsWith("search:") && (
                      <div className="browse-actions">
                        <button
                          className="primary-play"
                          title={`Play ${title}`}
                          onClick={() => {
                            if (state.route.startsWith("artist:"))
                              play({
                                command: "play_context",
                                context: `spotify:${state.route}`,
                              });
                            else {
                              const index = displayTracks.findIndex(playable);
                              if (index >= 0)
                                playTrack(displayTracks[index], index);
                            }
                          }}
                        >
                          <Play fill="currentColor" size={24} />
                        </button>
                        <button
                          className={`icon-button action-icon ${state.playback.shuffle_state ? "active" : ""}`}
                          title={
                            state.playback.shuffle_state
                              ? "Turn off shuffle"
                              : "Turn on shuffle"
                          }
                          onClick={() =>
                            play({
                              command: "shuffle",
                              enabled: !state.playback.shuffle_state,
                            })
                          }
                        >
                          <Shuffle size={28} />
                        </button>
                        {entity?.artist &&
                          (() => {
                            const artist = entity.artist;
                            const following =
                              !!state.saved[
                                artist.uri || `spotify:artist:${artist.id}`
                              ];
                            return (
                              <button
                                className="pill outline"
                                aria-pressed={following}
                                onClick={() => follow(artist)}
                              >
                                {following ? "Following" : "Follow"}
                              </button>
                            );
                          })()}
                        {collection && (
                          <div className="song-filters">
                            <small aria-live="polite">
                              {filtering
                                ? `${displayTracks.length} songs${state.more ? " · loading the rest…" : ""}`
                                : ""}
                            </small>
                            {songSearch || songQuery ? (
                              <label className="playlist-search">
                                <Search size={16} />
                                <input
                                  autoFocus
                                  aria-label="Search songs in this playlist"
                                  placeholder="Search in playlist"
                                  value={songQuery}
                                  onChange={(event) =>
                                    setSongQuery(event.target.value)
                                  }
                                  onBlur={() => setSongSearch(false)}
                                  onKeyDown={(event) => {
                                    if (event.key === "Escape") {
                                      setSongQuery("");
                                      setSongSearch(false);
                                    }
                                  }}
                                />
                              </label>
                            ) : (
                              <button
                                className="icon-button"
                                title="Search in playlist"
                                onClick={() => setSongSearch(true)}
                              >
                                <Search size={18} />
                              </button>
                            )}
                            <SongSortMenu
                              sort={songSort}
                              genre={songGenre}
                              age={songAge}
                              genres={songGenres}
                              onSort={setSongSort}
                              onGenre={setSongGenre}
                              onAge={setSongAge}
                            />
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                </>
              )}
              <div className={entity?.artist ? "artist-top" : "track-section"}>
                <section>
                  {entity?.artist && displayTracks.length > 0 && (
                    <div className="browse-section-title">
                      <h2>Popular</h2>
                    </div>
                  )}
                  {showTrackList && (
                    <>
                      <div
                        ref={trackTable}
                        className={`track-table ${collectionPlays ? "collection-table" : ""} ${collection ? "dated-table" : ""} ${entity?.album ? "album-table" : ""}`}
                      >
                        <div className="track-table-head">
                          <span>#</span>
                          <span>{headerSort("title", "Title")}</span>
                          <span>
                            {entity?.artist
                              ? "Streams"
                              : entity?.album
                                ? "Plays"
                                : headerSort("album", "Album")}
                          </span>
                          {collection && (
                            <span>{headerSort("added", "Date added")}</span>
                          )}
                          {collectionPlays && <span>Plays</span>}
                          <span>
                            <Clock3 size={16} />
                          </span>
                        </div>
                        {rows.before > 0 && (
                          <div
                            className="track-spacer"
                            style={{ height: rows.before }}
                            aria-hidden="true"
                          />
                        )}
                        {listedTracks
                          .slice(rows.start, rows.end)
                          .map((track, offset) => {
                            const index = rows.start + offset;
                            const current =
                              track.uri === state.playback.item?.uri;
                            return (
                              <div
                                title={
                                  track.unavailable
                                    ? "This song is not available on Spotify"
                                    : undefined
                                }
                                aria-selected={selected.has(track.uri)}
                                data-uri={track.uri}
                                className={`track-row ${current ? "playing" : ""} ${selected.has(track.uri) ? "selected" : ""} ${track.unavailable ? "unavailable" : ""} ${reorderable && dropAt === index ? "drop-before" : ""} ${reorderable && dropAt === displayTracks.length && index === displayTracks.length - 1 ? "drop-after" : ""}`}
                                key={`${track.uri}-${index}`}
                                draggable={reorderable}
                                onDragStart={(event) => {
                                  // Unfiltered playlist order: row = position.
                                  dragRow.current = index;
                                  event.dataTransfer.effectAllowed = "move";
                                }}
                                onDragOver={(event) => {
                                  if (dragRow.current === null) return;
                                  event.preventDefault();
                                  const box =
                                    event.currentTarget.getBoundingClientRect();
                                  setDropAt(
                                    event.clientY > box.top + box.height / 2
                                      ? index + 1
                                      : index,
                                  );
                                }}
                                onDrop={(event) => {
                                  event.preventDefault();
                                  const from = dragRow.current;
                                  dragRow.current = null;
                                  setDropAt(null);
                                  if (from === null || dropAt === null) return;
                                  // Dropping before `dropAt` in the old order.
                                  moveInPlaylist(
                                    from,
                                    dropAt > from ? dropAt - 1 : dropAt,
                                  );
                                }}
                                onDragEnd={() => {
                                  dragRow.current = null;
                                  setDropAt(null);
                                }}
                                onClick={(event) => {
                                  // Links and buttons keep their own clicks.
                                  if (
                                    (event.target as HTMLElement).closest(
                                      "a, button",
                                    )
                                  )
                                    return;
                                  selectRow(index, event);
                                }}
                                onDoubleClick={(event) => {
                                  // The title already played on its first click.
                                  if (
                                    !(event.target as HTMLElement).closest(
                                      ".track-name",
                                    )
                                  )
                                    playTrack(track, index);
                                }}
                                onMouseEnter={() => prefetchCanvas(track.uri)}
                                onContextMenu={(event) => {
                                  // Anywhere on the row opens the song menu;
                                  // artist and album links keep their own menus.
                                  const link = (
                                    event.target as HTMLElement
                                  ).closest<HTMLElement>(
                                    "[data-context-route]",
                                  );
                                  if (
                                    link &&
                                    !link.dataset.contextRoute?.startsWith(
                                      "track:",
                                    )
                                  )
                                    return;
                                  event.preventDefault();
                                  setEntityMenu(null);
                                  // The menu acts on the whole selection when
                                  // opened on one of its songs; otherwise the
                                  // song alone is selected, as in Spotify.
                                  const group =
                                    selected.has(track.uri) && selected.size > 1
                                      ? listedTracks.filter((item) =>
                                          selected.has(item.uri),
                                        )
                                      : undefined;
                                  if (!selected.has(track.uri)) {
                                    selectAnchor.current = index;
                                    setSelected(new Set([track.uri]));
                                  }
                                  setSongMenu({
                                    track,
                                    tracks: group,
                                    fromList: true,
                                    x: event.clientX,
                                    y: event.clientY,
                                  });
                                }}
                              >
                                <div className="track-number">
                                  <span>
                                    {current && state.playback.is_playing ? (
                                      <span className="equalizer">
                                        <i />
                                        <i />
                                        <i />
                                      </span>
                                    ) : (
                                      index + 1
                                    )}
                                  </span>
                                  <button
                                    className="row-play"
                                    disabled={track.unavailable}
                                    title={
                                      current && state.playback.is_playing
                                        ? "Pause"
                                        : "Play"
                                    }
                                    onClick={(event) => {
                                      event.stopPropagation();
                                      if (current && state.playback.is_playing)
                                        play({ command: "pause" });
                                      else playTrack(track, index);
                                    }}
                                  >
                                    {current && state.playback.is_playing ? (
                                      <Pause size={17} fill="currentColor" />
                                    ) : (
                                      <Play size={17} fill="currentColor" />
                                    )}
                                  </button>
                                </div>
                                <div className="track-cell">
                                  <Cover
                                    images={
                                      track.album.images.length
                                        ? track.album.images
                                        : entity?.album?.images
                                    }
                                  />
                                  <div>
                                    <button
                                      className="text-link track-name"
                                      onClick={(event) => {
                                        event.stopPropagation();
                                        selectRow(index, event);
                                        // Ctrl and Shift clicks only select.
                                        if (
                                          event.ctrlKey ||
                                          event.metaKey ||
                                          event.shiftKey
                                        )
                                          return;
                                        // The first click plays; the rest of a
                                        // double-click must not load it again.
                                        if (event.detail <= 1)
                                          playTrack(track, index);
                                      }}
                                    >
                                      {track.name}
                                    </button>
                                    <div className="track-artists">
                                      <ExplicitBadge track={track} />
                                      {!entity?.artist && (
                                        <ArtistLinks
                                          artists={track.artists}
                                          onArtist={openArtist}
                                        />
                                      )}
                                    </div>
                                  </div>
                                </div>
                                {entity?.album ? (
                                  <span
                                    className="track-streams"
                                    title="Spotify play count"
                                  >
                                    {entity.album_details?.streams[
                                      track.uri
                                    ]?.toLocaleString() ?? "—"}
                                  </span>
                                ) : (
                                  <button
                                    className="text-link album-link"
                                    data-context-route={
                                      (entity?.album || track.album).id
                                        ? `album:${(entity?.album || track.album).id}`
                                        : (
                                            entity?.album || track.album
                                          ).uri?.replace("spotify:", "")
                                    }
                                    data-context-name={
                                      (entity?.album || track.album).name
                                    }
                                    onClick={(event) => {
                                      event.stopPropagation();
                                      openAlbum(entity?.album || track.album);
                                    }}
                                  >
                                    {entity?.artist
                                      ? entity.overview?.popular[
                                          index
                                        ].streams?.toLocaleString()
                                      : track.album.name || entity?.album?.name}
                                  </button>
                                )}
                                {collection && (
                                  <span
                                    className="track-added"
                                    title={
                                      track.added_at
                                        ? new Date(
                                            track.added_at,
                                          ).toLocaleString()
                                        : undefined
                                    }
                                  >
                                    {addedLabel(track.added_at)}
                                  </span>
                                )}
                                {collectionPlays && (
                                  <span
                                    className="track-streams"
                                    title={
                                      state.facts[track.uri]?.plays == null
                                        ? "Spotify play count unavailable"
                                        : "Spotify play count"
                                    }
                                  >
                                    {state.facts[
                                      track.uri
                                    ]?.plays?.toLocaleString() ?? "—"}
                                  </span>
                                )}
                                <div className="row-end">
                                  <span>
                                    {track.duration_ms > 0
                                      ? formatTime(track.duration_ms)
                                      : ""}
                                  </span>
                                  <button
                                    className="icon-button"
                                    title="Song options"
                                    onClick={(event) => {
                                      event.stopPropagation();
                                      setSongMenu({
                                        track,
                                        fromList: true,
                                        x: event.clientX - 160,
                                        y: event.clientY,
                                      });
                                    }}
                                  >
                                    <MoreHorizontal size={19} />
                                  </button>
                                </div>
                              </div>
                            );
                          })}
                        {rows.after > 0 && (
                          <div
                            className="track-spacer"
                            style={{ height: rows.after }}
                            aria-hidden="true"
                          />
                        )}
                      </div>
                      {entity?.artist && displayTracks.length > 5 && (
                        <button
                          className="text-link see-more"
                          onClick={() => setPopularMore(!popularMore)}
                        >
                          {popularMore ? "Show less" : "See more"}
                        </button>
                      )}
                    </>
                  )}
                </section>
                {entity?.artist && (likedHere > 0 || entity.overview?.pick) && (
                  <aside className="artist-liked">
                    {likedHere > 0 && (
                      <div className="artist-aside-block">
                        <h2>You Liked</h2>
                        <div className="artist-liked-row">
                          <div className="liked-artist-image">
                            <Cover images={entity.images} />
                            <Heart
                              className="liked-heart"
                              fill="currentColor"
                              size={25}
                            />
                          </div>
                          <span>
                            <button
                              className="artist-liked-count"
                              onClick={() =>
                                openLikedByArtist(
                                  artistId,
                                  entity.artist?.name || entity.title,
                                )
                              }
                            >
                              {likedHere} song{likedHere === 1 ? "" : "s"}
                            </button>
                            <small>By {entity.artist.name}</small>
                          </span>
                        </div>
                      </div>
                    )}
                    {entity.overview?.pick && (
                      <div className="artist-aside-block">
                        <h2>Artist pick</h2>
                        <button
                          className="artist-pick"
                          data-context-route={entity.overview.pick.route}
                          data-context-name={entity.overview.pick.name}
                          onClick={() =>
                            entity.overview!.pick!.kind === "track"
                              ? homePlay(entity.overview!.pick!)
                              : navigate(entity.overview!.pick!.route)
                          }
                        >
                          <Cover images={entity.overview.pick.images} />
                          <span>
                            <span className="artist-pick-posted">
                              {(entity.overview.avatar_image ||
                                imageOf(entity.images)) && (
                                <img
                                  src={
                                    entity.overview.avatar_image ||
                                    imageOf(entity.images)
                                  }
                                  alt=""
                                />
                              )}
                              Posted By {entity.artist.name}
                            </span>
                            <strong>{entity.overview.pick.name}</strong>
                            <small>{entity.overview.pick.subtitle}</small>
                          </span>
                        </button>
                      </div>
                    )}
                  </aside>
                )}
              </div>
              {entity?.album && (
                <section className="album-release-details">
                  <p>
                    {entity.album.release_date?.length === 10
                      ? new Date(
                          `${entity.album.release_date}T00:00:00`,
                        ).toLocaleDateString(undefined, {
                          year: "numeric",
                          month: "long",
                          day: "numeric",
                        })
                      : entity.album.release_date}
                  </p>
                  {(entity.album.copyrights?.length
                    ? entity.album.copyrights
                    : entity.album_details?.copyrights || []
                  ).map((credit, index) => (
                    <small key={index}>{credit.text}</small>
                  ))}
                </section>
              )}
              {(entity?.artist || !!entity?.album_details?.more?.length) && (
                <>
                  <div className="browse-section-title">
                    <h2>
                      {entity?.artist
                        ? "Discography"
                        : `More by ${entity?.album?.artists?.[0]?.name ?? "this artist"}`}
                    </h2>
                    <button
                      className="text-link muted"
                      onClick={() =>
                        entity?.artist
                          ? navigate(`discography:${artistId}`)
                          : setAllAlbums(!allAlbums)
                      }
                    >
                      {allAlbums ? "Show less" : "Show all"}
                    </button>
                  </div>
                  <div className="discography-filters">
                    {[
                      ["all", "Popular releases"],
                      ["album", "Albums"],
                      ["single", "Singles and EPs"],
                      ...(entity?.overview?.discography?.compilations.length ||
                      (entity?.artist ? entity.albums : []).some(
                        (album) =>
                          albumKind(album.album_type) === "compilation",
                      )
                        ? [["compilation", "Compilations"]]
                        : []),
                    ].map(([value, label]) => (
                      <button
                        key={value}
                        className={`pill ${albumFilter === value ? "active" : ""}`}
                        onClick={() => setAlbumFilter(value)}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                  <div className={`album-grid ${allAlbums ? "all" : ""}`}>
                    {(entity?.artist
                      ? discographyReleases(
                          entity.albums,
                          entity.overview?.discography,
                          albumFilter,
                        )
                      : (entity?.album_details?.more || []).filter((album) =>
                          albumMatches(album.album_type, albumFilter),
                        )
                    )
                      .slice(0, allAlbums ? undefined : 10)
                      .map((album) => (
                        <button
                          key={album.id}
                          className="album-tile"
                          onClick={() => openAlbum(album)}
                          {...prefetchOn(`album:${album.id}`)}
                        >
                          <Cover images={album.images} />
                          <strong>{album.name}</strong>
                          <small>
                            {albumFilter === "all" &&
                            album.uri ===
                              entity?.overview?.discography?.latest?.uri
                              ? "Latest Release"
                              : album.release_date?.slice(0, 4)}{" "}
                            · {albumTypeLabel(album.album_type)}
                          </small>
                        </button>
                      ))}
                  </div>
                </>
              )}
              {entity?.overview &&
                entity.artist &&
                (() => {
                  const overview = entity.overview;
                  const sections = overview.sections ?? [];
                  const shelf = (section: (typeof sections)[number]) => (
                    <div className="artist-shelf" key={section.uri}>
                      <Shelf
                        section={section}
                        navigate={openCard}
                        play={homePlay}
                        ready={state.native_ready}
                        onShowAll={() =>
                          navigate(
                            `artist-shelf:${artistId}:${sections.indexOf(section)}`,
                          )
                        }
                      />
                    </div>
                  );
                  const photo =
                    overview.gallery_image ||
                    overview.header_image ||
                    overview.avatar_image;
                  return (
                    <>
                      {sections
                        .filter((section) =>
                          section.title.startsWith("Featuring "),
                        )
                        .map(shelf)}
                      <section className="artist-about">
                        <h2>About</h2>
                        <button
                          className="artist-about-card"
                          style={
                            photo
                              ? {
                                  backgroundImage: `linear-gradient(transparent 35%, #000b), url("${photo}")`,
                                }
                              : undefined
                          }
                          aria-label={`About ${entity.artist.name}`}
                          onClick={() => {
                            setAboutPhoto(0);
                            setAboutOpen(true);
                          }}
                        >
                          {overview.world_rank != null && (
                            <span className="artist-rank">
                              <strong>#{overview.world_rank}</strong>
                              in the world
                            </span>
                          )}
                          <span className="artist-about-copy">
                            {overview.monthly_listeners != null && (
                              <strong>
                                {overview.monthly_listeners.toLocaleString()}{" "}
                                monthly listeners
                              </strong>
                            )}
                            {overview.biography && (
                              <span>{overview.biography}</span>
                            )}
                          </span>
                        </button>
                      </section>
                      {sections
                        .filter(
                          (section) => !section.title.startsWith("Featuring "),
                        )
                        .map(shelf)}
                      {aboutOpen && (
                        <div
                          className="modal-backdrop"
                          onClick={() => setAboutOpen(false)}
                        >
                          <section
                            className="dialog artist-about-dialog"
                            role="dialog"
                            aria-label={`About ${entity.artist.name}`}
                            onClick={(event) => event.stopPropagation()}
                          >
                            <button
                              className="icon-button artist-about-close"
                              aria-label="Close"
                              onClick={() => setAboutOpen(false)}
                            >
                              <X size={20} />
                            </button>
                            {(() => {
                              const photos = overview.gallery?.length
                                ? overview.gallery
                                : photo
                                  ? [photo]
                                  : [];
                              if (!photos.length) return null;
                              const shown = aboutPhoto % photos.length;
                              return (
                                <div className="artist-about-gallery">
                                  <img
                                    src={photos[shown]}
                                    alt={`${entity.artist.name}, photo ${shown + 1} of ${photos.length}`}
                                  />
                                  {photos.length > 1 && (
                                    <>
                                      {shown > 0 && (
                                        <button
                                          className="gallery-step previous"
                                          aria-label="Previous photo"
                                          onClick={() =>
                                            setAboutPhoto(
                                              (shown + photos.length - 1) %
                                                photos.length,
                                            )
                                          }
                                        >
                                          <ChevronLeft size={24} />
                                        </button>
                                      )}
                                      <button
                                        className="gallery-step next"
                                        aria-label="Next photo"
                                        onClick={() =>
                                          setAboutPhoto(
                                            (shown + 1) % photos.length,
                                          )
                                        }
                                      >
                                        <ChevronRight size={24} />
                                      </button>
                                    </>
                                  )}
                                </div>
                              );
                            })()}
                            <div className="artist-about-body">
                              <aside className="artist-about-facts">
                                {overview.world_rank != null && (
                                  <span className="artist-rank">
                                    <strong>#{overview.world_rank}</strong>
                                    in the world
                                  </span>
                                )}
                                {overview.followers != null && (
                                  <p className="artist-fact">
                                    <strong>
                                      {overview.followers.toLocaleString()}
                                    </strong>
                                    Followers
                                  </p>
                                )}
                                {overview.monthly_listeners != null && (
                                  <p className="artist-fact">
                                    <strong>
                                      {overview.monthly_listeners.toLocaleString()}
                                    </strong>
                                    Monthly Listeners
                                  </p>
                                )}
                                {overview.top_cities?.map((city) => (
                                  <p
                                    className="artist-city"
                                    key={`${city.city}-${city.country}`}
                                  >
                                    <strong>
                                      {city.city}
                                      {city.country ? `, ${city.country}` : ""}
                                    </strong>
                                    {city.listeners.toLocaleString()} listeners
                                  </p>
                                ))}
                              </aside>
                              <div className="artist-about-text">
                                {(overview.biography_html ||
                                  overview.biography) && (
                                  <p>
                                    <SpotifyText
                                      text={
                                        overview.biography_html ||
                                        overview.biography
                                      }
                                      navigate={(route) => {
                                        setAboutOpen(false);
                                        navigate(route);
                                      }}
                                    />
                                  </p>
                                )}
                                <button
                                  className="text-link muted"
                                  onClick={() =>
                                    void call("open_link", {
                                      url: overview.source_url,
                                    })
                                  }
                                >
                                  Source: Spotify · Updated{" "}
                                  {new Date(
                                    overview.fetched_at * 1000,
                                  ).toLocaleString()}
                                </button>
                              </div>
                            </div>
                          </section>
                        </div>
                      )}
                    </>
                  );
                })()}
              {state.route.startsWith("artist:") && !entity?.overview && (
                <div className="artist-loading" role="status">
                  <LoaderCircle className="spin" size={18} />
                  Loading artist details…
                </div>
              )}
              {state.loading &&
                !state.tracks.length &&
                !state.route.startsWith("artist:") && (
                  <div
                    className="skeleton-rows"
                    role="status"
                    aria-label={
                      state.route.startsWith("search:")
                        ? "Searching Spotify"
                        : "Loading"
                    }
                  >
                    {Array.from({ length: 8 }, (_, index) => (
                      <div className="skeleton-row" key={index}>
                        <i />
                        <i />
                        <div>
                          <i />
                          <i />
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              {!state.loading && state.pageError && (
                <div className="empty-page">
                  <h2>Couldn’t load this page</h2>
                  <p>{state.pageError}</p>
                  <button
                    className="pill"
                    onClick={() => navigate(state.route, false)}
                  >
                    Try again
                  </button>
                </div>
              )}
              {state.route !== "home" &&
                !state.route.startsWith("artist:") &&
                !state.loading &&
                !state.pageError &&
                !state.tracks.length &&
                (!state.route.startsWith("search:") ||
                  (!state.search_playlists_loading &&
                    !state.search_playlists.length)) &&
                !entity?.artist && (
                  <div className="empty-page">
                    <Search size={32} />
                    <h2>
                      {state.route.startsWith("search:")
                        ? "No songs found"
                        : "Nothing here yet"}
                    </h2>
                    <p>
                      {state.route.startsWith("search:")
                        ? "Try another song, album or artist name."
                        : "Choose something from your library."}
                    </p>
                  </div>
                )}
              <div
                ref={pageSentinel}
                className="page-sentinel"
                aria-hidden="true"
              />
            </div>
          )}
        </main>
        {sidebar && (
          <>
            <div className="panel-resizer" onPointerDown={resizeRight} />
            {panel === "queue" ? (
              <QueuePanel
                state={state}
                onPlayer={play}
                onClose={() => setPanel("now")}
                jam={jam}
                jamInvite={jamInvite}
                onJamInvite={setJamInvite}
                onJamJoin={() =>
                  rpc<string>("read_clipboard")
                    .catch(() => "")
                    .then((text) =>
                      isJamLink(text)
                        ? joinJam(text)
                        : notify(
                            "Copy a Jam link from Spotify or Lyssna, then press Join a Jam.",
                            true,
                          ),
                    )
                }
                onJamCopy={() =>
                  jam && copyText(jam.invite_url, "Jam link copied.")
                }
                onJamEnd={() => {
                  if (!jam) return;
                  rpc("jam", {
                    action: jam.is_owner ? "end" : "leave",
                    id: jam.session_id,
                  })
                    .then(() => {
                      setJam(null);
                      setJamInvite(false);
                    })
                    .catch((error) => notify(String(error), true));
                }}
                onJamControl={(allowed) => {
                  if (!jam) return;
                  setJam({ ...jam, queue_only: !allowed });
                  rpc<Jam | null>("jam", { action: "control", allowed })
                    .then((next) => next && setJam(next))
                    .catch((error) => {
                      setJam(jam);
                      notify(String(error), true);
                    });
                }}
              />
            ) : (
              <NowPlaying
                state={state}
                canvasInLyrics={center === "lyrics" && canvasInLyrics}
                onPlayer={play}
                onAlbum={openAlbum}
                onArtist={openArtist}
                onLyrics={() => setCenter("lyrics")}
                onQueue={() => setPanel("queue")}
                onFollow={follow}
                onSave={save}
                showLyricsPreview={
                  center !== "lyrics" &&
                  !state.settings.lyrics_hide_npv &&
                  !(
                    state.settings.lyrics_hide_npv_empty &&
                    !state.lyrics?.lines.length
                  )
                }
              />
            )}
          </>
        )}
      </div>
      {state.creator_profile_open && (
        // The profile sheet is a separate window; clicking here moves focus
        // back to this window, which closes it.
        <div className="creator-profile-dim" aria-hidden="true" />
      )}
      {djRequest && state.profile && state.native_ready && (
        <DJRequest
          key={state.profile.id}
          account={state.profile.id}
          onClose={() => setDJRequest(false)}
          onSuccess={(transcript) =>
            notify(transcript ? `DJ heard: “${transcript}”` : "DJ request sent")
          }
        />
      )}
      <PlayerBar
        state={state}
        onPlayer={play}
        onDJRequest={() => setDJRequest(true)}
        onToggle={toggle}
        onAlbum={openAlbum}
        onArtist={openArtist}
        onSave={save}
        onLyrics={() => setCenter(center === "lyrics" ? "browse" : "lyrics")}
        onQueue={toggleQueue}
        onDevices={() => {
          setDeviceMenu(!deviceMenu);
          if (!deviceMenu) void call("refresh");
        }}
        onSidebar={() =>
          animateLayout(
            () => {
              if (sidebar && panel === "queue") setPanel("now");
              else {
                setSidebar(!sidebar);
                setPanel("now");
              }
            },
            "panels",
            state.settings.reduce_motion,
          )
        }
        onFullscreen={() => void fullscreen()}
        lyricsActive={center === "lyrics"}
        queueActive={sidebar && panel === "queue"}
        sidebarActive={sidebar && panel === "now"}
      />
      {deviceMenu && (
        <div className="device-menu popover">
          <h3>Connect to a device</h3>
          <button
            className={target === "native" ? "selected" : ""}
            onClick={() => {
              if (target !== "native")
                void call("player", {
                  request: { command: "transfer", uri: "native" },
                  target: "native",
                });
              setTarget("native");
              setDeviceMenu(false);
            }}
          >
            <Speaker size={21} />
            <div>
              <strong>This computer</strong>
              <small>
                {state.native_ready
                  ? "Ready for native playback"
                  : state.native_status}
              </small>
            </div>
            {target === "native" && <Check size={18} />}
          </button>
          {state.devices
            .filter((device) => device.name !== state.local_device_name)
            .map((device) => (
              <button
                key={device.id}
                className={target === device.id ? "selected" : ""}
                onClick={() => {
                  setTarget(device.id || "native");
                  setDeviceMenu(false);
                  if (device.id)
                    void call("player", {
                      request: { command: "transfer", uri: device.id },
                      target: device.id,
                    });
                }}
              >
                <Speaker size={20} />
                <div>
                  <strong>{device.name}</strong>
                  <small>
                    {device.is_active
                      ? "Active device"
                      : device.is_offline
                        ? "Standby · start Spotify on the speaker first"
                        : "Spotify Connect"}
                  </small>
                </div>
              </button>
            ))}
        </div>
      )}
      {songMenu && (
        <>
          <div
            className="popover-dismiss"
            onClick={() => {
              setSongMenu(null);
              setPicker(null);
            }}
          />
          <div
            ref={songMenuElement}
            className="song-menu popover"
            role="menu"
            aria-label="Song actions"
            style={{
              left: songMenu.x,
              top: songMenu.y,
            }}
          >
            {[
              [
                "Play",
                Play,
                () =>
                  songMenu.fromList
                    ? playTrack(songMenu.track)
                    : play({
                        command: "play",
                        uri: songMenu.track.uri,
                        track: songMenu.track,
                      }),
              ],
              [
                state.saved[songMenu.track.uri]
                  ? "Remove from Liked Songs"
                  : "Save to Liked Songs",
                Heart,
                () => save(songMenu.track),
              ],
              [
                "Add to queue",
                ListMusic,
                () => {
                  for (const track of songMenu.tracks ?? [songMenu.track])
                    play({ command: "enqueue", uri: track.uri });
                },
              ],
              ...(jam
                ? [
                    [
                      "Invite to Jam",
                      Users,
                      () => {
                        setJamInvite(true);
                        setPanel("queue");
                        setSidebar(true);
                        copyText(jam.invite_url, "Jam link copied.");
                      },
                    ],
                  ]
                : [
                    [
                      "Start a Jam",
                      Users,
                      () => {
                        // Like Spotify, the Jam starts on this song.
                        if (state.playback.item?.uri !== songMenu.track.uri) {
                          if (songMenu.fromList) playTrack(songMenu.track);
                          else
                            play({
                              command: "play",
                              uri: songMenu.track.uri,
                              track: songMenu.track,
                            });
                        }
                        rpc<Jam>("jam", { action: "start" })
                          .then((started) => {
                            setJam(started);
                            setJamInvite(true);
                            setPanel("queue");
                            setSidebar(true);
                            copyText(started.invite_url, "Jam link copied.");
                          })
                          .catch((error) => notify(String(error), true));
                      },
                    ],
                  ]),
              ...(songMenu.queued
                ? [
                    [
                      "Remove from queue",
                      Trash2,
                      () =>
                        play({
                          command: "remove_from_queue",
                          uri: songMenu.track.uri,
                        }),
                    ],
                  ]
                : []),
              ["Add to playlist", Plus, () => {}],
              ...(songMenu.fromList &&
              state.playlists.some(
                (item) =>
                  `playlist:${item.id}` === state.route &&
                  (item.owner.id === state.profile?.id || item.collaborative),
              )
                ? [
                    [
                      "Remove from this playlist",
                      Trash2,
                      () => removeFromPlaylist(songMenu.track),
                    ],
                  ]
                : []),
              ["Open album", Music2, () => openAlbum(songMenu.track.album)],
              [
                "Go to artist",
                Music2,
                () => {
                  const artist = songMenu.track.artists[0];
                  if (artist) openArtist(artist);
                },
              ],
              [
                "Copy clean link",
                Copy,
                () =>
                  copyText(
                    `https://open.spotify.com/track/${songMenu.track.id}`,
                    "Song link copied.",
                  ),
              ],
              [
                "Copy song & artist name",
                Copy,
                () =>
                  copyText(
                    songAndArtist(songMenu.track),
                    "Song and artist copied.",
                  ),
              ],
              songMenu.tracks
                ? [
                    "Copy song IDs",
                    Copy,
                    () => copyText(trackIds(songMenu.tracks!), "IDs copied."),
                  ]
                : [
                    "Copy song ID",
                    Copy,
                    () =>
                      copyText(
                        songMenu.track.id || songMenu.track.uri.split(":")[2],
                        "Song ID copied.",
                      ),
                  ],
              [
                "Copy embed code",
                Code,
                () =>
                  copyText(
                    embedCode(
                      "track",
                      songMenu.track.id || songMenu.track.uri.split(":")[2],
                    ),
                    "Embed code copied.",
                  ),
              ],
            ].map(([label, Icon, action]) => {
              const ItemIcon = Icon as typeof Play;
              // Add to playlist opens a flyout beside the menu, like Spotify.
              const submenu = label === "Add to playlist";
              const openFlyout = (button: HTMLElement) => {
                const rect = button.getBoundingClientRect(),
                  right = rect.right + 300 + 8 <= window.innerWidth;
                setPicker({
                  tracks: songMenu.tracks ?? [songMenu.track],
                  x: right ? rect.right + 4 : rect.left - 304,
                  y: rect.top - 8,
                });
              };
              return (
                <button
                  key={String(label)}
                  role="menuitem"
                  aria-haspopup={submenu ? "menu" : undefined}
                  aria-expanded={submenu ? !!picker : undefined}
                  className={submenu && picker ? "active" : undefined}
                  onMouseEnter={(event) =>
                    submenu ? openFlyout(event.currentTarget) : setPicker(null)
                  }
                  onClick={(event) => {
                    if (submenu) return openFlyout(event.currentTarget);
                    (action as () => void)();
                    setSongMenu(null);
                  }}
                >
                  <ItemIcon size={17} />
                  {String(label)}
                  {submenu && (
                    <ChevronRight size={17} className="menu-chevron" />
                  )}
                </button>
              );
            })}
          </div>
        </>
      )}
      {entityMenu &&
        (() => {
          const menu = entityMenu,
            current = stateRef.current;
          const playlist =
            menu.kind === "playlist"
              ? libraryItems(current).find((item) => item.id === menu.id) ||
                playlistForManualPlay(current, {
                  command: "play_home",
                  context: `spotify:playlist:${menu.id}`,
                }) || {
                  id: menu.id,
                  name: menu.name,
                  images: [],
                  owner: { id: "" },
                  collaborative: false,
                }
              : undefined;
          const actions: { label: string; run: () => void }[] = [];
          if (menu.kind === "liked")
            actions.push({
              label: "Open Liked Songs",
              run: () => navigate("liked"),
            });
          else {
            actions.push({
              label: `Go to ${menu.kind}`,
              run: () =>
                menu.id === DJ_ID
                  ? play({ command: "dj" })
                  : navigate(`${menu.kind}:${menu.id}`),
            });
            actions.push({
              label: "Play",
              run: () =>
                play(
                  menu.id === DJ_ID
                    ? { command: "dj" }
                    : {
                        command: "play_home",
                        context: `spotify:${menu.kind}:${menu.id}`,
                      },
                ),
            });
            if (playlist)
              actions.push({
                label: accountOrder(current).pinned.includes(playlist.id)
                  ? "Unpin playlist"
                  : "Pin playlist",
                run: () => togglePin(playlist),
              });
            const saved =
              menu.kind === "playlist" &&
              current.playlists.find((item) => item.id === menu.id);
            if (
              saved &&
              ((current.library_folders?.length ?? 0) > 0 ||
                !!saved.folders?.length)
            )
              actions.push({
                label: "Move to folder",
                run: () =>
                  setFolderPicker({ playlist: saved.id, x: menu.x, y: menu.y }),
              });
            if (saved) {
              const own = saved.owner.id === current.profile?.id;
              if (own)
                actions.push({
                  label: "Edit details",
                  run: () => openEditDialog(saved),
                });
              actions.push({
                label: own ? "Delete" : "Remove from Your Library",
                run: () =>
                  setRemoveDialog({ id: saved.id, name: saved.name, own }),
              });
            }
            if (menu.kind === "artist")
              actions.push({
                label: current.saved[`spotify:artist:${menu.id}`]
                  ? "Unfollow artist"
                  : "Follow artist",
                run: () =>
                  follow({
                    id: menu.id,
                    name: menu.name,
                    uri: `spotify:artist:${menu.id}`,
                  }),
              });
            actions.push({
              label: `Copy link to ${menu.kind}`,
              run: () =>
                copyText(
                  `https://open.spotify.com/${menu.kind}/${menu.id}`,
                  "Link copied.",
                ),
            });
            const kind = menu.kind;
            actions.push({
              label: "Copy embed code",
              run: () =>
                copyText(embedCode(kind, menu.id), "Embed code copied."),
            });
          }
          return (
            <>
              <div
                className="popover-dismiss"
                onClick={() => setEntityMenu(null)}
              />
              <div
                ref={entityMenuElement}
                role="menu"
                aria-label={`${menu.kind} actions`}
                className="song-menu popover entity-menu"
                style={{
                  left: menu.x,
                  top: menu.y,
                }}
                onKeyDown={(event) => {
                  if (
                    !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)
                  )
                    return;
                  event.preventDefault();
                  const buttons = Array.from(
                    event.currentTarget.querySelectorAll<HTMLButtonElement>(
                      "button",
                    ),
                  );
                  const index = buttons.indexOf(
                    document.activeElement as HTMLButtonElement,
                  );
                  buttons[
                    event.key === "Home"
                      ? 0
                      : event.key === "End"
                        ? buttons.length - 1
                        : (index +
                            (event.key === "ArrowDown" ? 1 : -1) +
                            buttons.length) %
                          buttons.length
                  ]?.focus();
                }}
              >
                {actions.map((action) => (
                  <button
                    role="menuitem"
                    key={action.label}
                    onClick={() => {
                      action.run();
                      setEntityMenu(null);
                    }}
                  >
                    {action.label}
                  </button>
                ))}
              </div>
            </>
          );
        })()}
      {picker && (
        <>
          {!songMenu && (
            <div className="popover-dismiss" onClick={() => setPicker(null)} />
          )}
          <PlaylistFlyout
            key={`${picker.x}:${picker.y}`}
            x={picker.x}
            y={picker.y}
            playlists={inChosenFolders(
              state.playlists.filter(
                (playlist) =>
                  playlist.owner.id === state.profile?.id ||
                  playlist.collaborative,
              ),
              plus.playlist_folders,
            )}
            onPick={(playlist) => {
              void call("add_tracks", { playlist, tracks: picker.tracks });
              setPicker(null);
              setSongMenu(null);
            }}
            onCreate={() => {
              const own = state.playlists.filter(
                (playlist) => playlist.owner.id === state.profile?.id,
              ).length;
              void call("create_playlist", {
                name:
                  picker.tracks.length === 1
                    ? picker.tracks[0].name
                    : `My Playlist #${own + 1}`,
                tracks: picker.tracks,
              });
              setPicker(null);
              setSongMenu(null);
            }}
          />
        </>
      )}
      {folderMenu && (
        <>
          <div
            className="popover-dismiss"
            onClick={() => setFolderMenu(null)}
          />
          <div
            role="menu"
            aria-label="Folder actions"
            className="song-menu popover entity-menu"
            style={{
              left: Math.max(
                8,
                Math.min(folderMenu.x, window.innerWidth - 250),
              ),
              top: Math.max(
                8,
                Math.min(folderMenu.y, window.innerHeight - 160),
              ),
            }}
          >
            <button
              role="menuitem"
              onClick={() => {
                setRenameFolder({
                  folder: folderMenu.folder,
                  name: folderMenu.folder.name,
                });
                setFolderMenu(null);
              }}
            >
              Rename
            </button>
            <button
              role="menuitem"
              onClick={() => {
                setDeleteFolder(folderMenu.folder);
                setFolderMenu(null);
              }}
            >
              Delete folder
            </button>
          </div>
        </>
      )}
      {state.profile && deleteFolder && (
        <div className="modal-backdrop" onClick={() => setDeleteFolder(null)}>
          <section
            className="dialog"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="delete-folder-title"
            aria-describedby="delete-folder-description"
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => {
              if (event.key !== "Tab") return;
              const buttons = event.currentTarget.querySelectorAll("button");
              const first = buttons[0],
                last = buttons[buttons.length - 1];
              if (event.shiftKey && document.activeElement === first) {
                event.preventDefault();
                last?.focus();
              } else if (!event.shiftKey && document.activeElement === last) {
                event.preventDefault();
                first?.focus();
              }
            }}
          >
            <h2 id="delete-folder-title">Delete folder?</h2>
            <p id="delete-folder-description">
              Delete &quot;{deleteFolder.name}&quot; from your Spotify library?
              Playlists and subfolders inside will move up one level and remain
              in Your Library.
            </p>
            <div className="inline-actions">
              <button
                className="pill"
                autoFocus
                onClick={() => setDeleteFolder(null)}
              >
                Cancel
              </button>
              <button
                className="pill primary"
                onClick={() => {
                  void call("delete_folder", { id: deleteFolder.id });
                  setDeleteFolder(null);
                }}
              >
                Delete folder
              </button>
            </div>
          </section>
        </div>
      )}
      {state.profile && folderPicker && folderPlaylist && (
        <>
          <div
            className="popover-dismiss"
            onClick={() => setFolderPicker(null)}
          />
          <div
            role="menu"
            aria-label="Move to folder"
            className="song-menu popover entity-menu folder-picker"
            style={{
              left: Math.max(
                8,
                Math.min(folderPicker.x, window.innerWidth - 270),
              ),
              top: Math.max(
                8,
                Math.min(folderPicker.y, window.innerHeight - 360),
              ),
            }}
          >
            {!!folderPlaylist.folders?.length && (
              <button
                role="menuitem"
                onClick={() => {
                  void call("move_to_folder", {
                    playlist: folderPlaylist.id,
                    folder: null,
                  });
                  setFolderPicker(null);
                }}
              >
                Remove from folder
              </button>
            )}
            {(state.library_folders ?? []).map((folder) => {
              const current = folderPlaylist.folders?.at(-1)?.id === folder.id;
              return (
                <button
                  role="menuitem"
                  key={folder.id}
                  disabled={current}
                  style={{ paddingLeft: 12 + folder.parents.length * 16 }}
                  onClick={() => {
                    void call("move_to_folder", {
                      playlist: folderPlaylist.id,
                      folder: folder.id,
                    });
                    setFolderPicker(null);
                  }}
                >
                  <Folder size={16} />
                  {folder.name}
                </button>
              );
            })}
          </div>
        </>
      )}
      {renameFolder && (
        <div className="modal-backdrop" onClick={() => setRenameFolder(null)}>
          <form
            className="dialog edit-playlist"
            aria-labelledby="rename-folder-title"
            onClick={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              if (!renameFolder.name.trim()) return;
              void call("rename_folder", {
                id: renameFolder.folder.id,
                name: renameFolder.name.trim(),
              });
              setRenameFolder(null);
            }}
          >
            <h2 id="rename-folder-title">Rename folder</h2>
            <label>
              <span>Name</span>
              <input
                className="field"
                autoFocus
                maxLength={100}
                value={renameFolder.name}
                onChange={(event) =>
                  setRenameFolder({ ...renameFolder, name: event.target.value })
                }
              />
            </label>
            <div className="inline-actions">
              <button
                type="button"
                className="pill"
                onClick={() => setRenameFolder(null)}
              >
                Cancel
              </button>
              <button
                type="submit"
                className="pill primary"
                disabled={!renameFolder.name.trim()}
              >
                Save
              </button>
            </div>
          </form>
        </div>
      )}
      {about && (
        <AboutDialog
          updateStatus={state.update_status}
          updateVersion={state.update_version}
          onUpdate={() => void call("app_update", { action: "install" })}
          onNotice={notify}
          onClose={() => setAbout(false)}
        />
      )}
      {state.profile && settingsTab && (
        <SettingsWindow
          state={state}
          initialTab={settingsTab}
          onSettings={updateSettings}
          onLoadTtml={() => {
            setSettingsTab(null);
            setTtmlOpen("load");
          }}
          onDatabase={() => {
            setSettingsTab(null);
            setTtmlOpen("database");
          }}
          onOpen={(url) => void call("open_link", { url })}
          onClose={() => setSettingsTab(null)}
          onCall={call}
          onNotice={notify}
          onManageAnimator={() => {
            setSettingsTab(null);
            manageAnimator.current();
          }}
        />
      )}
      {state.profile && animatorPanel && (
        <AnimatorPanel
          animator={state.settings.lyrics_animator}
          onChange={(lyrics_animator) => updateSettings({ lyrics_animator })}
          onBack={() => {
            closeAnimatorPanel();
            setSettingsTab("Effects");
          }}
          onClose={closeAnimatorPanel}
        />
      )}
      {ttmlOpen && (
        <LoadTtml
          initialView={ttmlOpen}
          song={state.playback.item?.name ?? "No song playing"}
          onLoad={(file, mode) => importFile(file, mode)}
          onReset={() => {
            void call("reset_lyrics").then(() =>
              notify("Imported lyrics cleared for this song."),
            );
            setTtmlOpen(false);
          }}
          onGuide={() =>
            void call("open_link", {
              url: "https://lyrprep.spicylyrics.org/guide",
            })
          }
          listSaved={listSavedLyrics}
          deleteSaved={async (key) => {
            await call("delete_saved_lyrics", { key });
          }}
          onPlay={(uri) => play({ command: "play", uri })}
          onDownload={(item) =>
            void rpc<[string, string]>("saved_lyrics_file", {
              key: item.key,
            }).then(
              ([filename, contents]) => {
                const link = document.createElement("a");
                link.href = URL.createObjectURL(
                  new Blob([contents], {
                    type:
                      item.kind === "ttml"
                        ? "application/ttml+xml"
                        : "text/plain",
                  }),
                );
                link.download = filename;
                link.click();
                setTimeout(() => URL.revokeObjectURL(link.href), 10_000);
              },
              (error) => notify(String(error), true),
            )
          }
          onClose={() => setTtmlOpen(false)}
        />
      )}
      {editDialog && (
        <div className="modal-backdrop" onClick={() => setEditDialog(null)}>
          <form
            className="dialog edit-playlist"
            aria-labelledby="edit-playlist-title"
            onClick={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              if (!editDialog.name.trim()) return;
              void call("edit_playlist", {
                playlist: editDialog.playlist,
                name: editDialog.name,
                description: editDialog.description,
                cover: editDialog.cover?.split(",", 2)[1] ?? null,
              });
              const cover = editDialog.cover;
              if (cover) {
                const id = editDialog.playlist.id;
                setCoverPreviews((current) => ({ ...current, [id]: cover }));
                dispatch({
                  type: "library",
                  playlists: state.playlists.map((item) =>
                    item.id === id
                      ? { ...item, images: [{ url: cover }] }
                      : item,
                  ),
                });
              }
              setEditDialog(null);
            }}
          >
            <h2 id="edit-playlist-title">Edit details</h2>
            <div className="edit-playlist-body">
              <label className="edit-playlist-cover" title="Choose photo">
                {editDialog.cover ? (
                  <img src={editDialog.cover} alt="" />
                ) : (
                  <Cover images={editDialog.playlist.images} />
                )}
                <span className="edit-playlist-cover-hint">
                  <ImagePlus size={28} />
                  Choose photo
                </span>
                <input
                  type="file"
                  accept="image/jpeg,image/png,image/webp"
                  aria-label="Choose playlist cover"
                  onChange={(event) => {
                    const file = event.target.files?.[0];
                    event.target.value = "";
                    if (!file) return;
                    coverJpeg(file).then(
                      (cover) =>
                        setEditDialog((current) =>
                          current ? { ...current, cover } : current,
                        ),
                      (error) => notify(String(error.message ?? error), true),
                    );
                  }}
                />
              </label>
              <div className="edit-playlist-fields">
                <label>
                  <span>Name</span>
                  <input
                    className="field"
                    autoFocus
                    maxLength={100}
                    value={editDialog.name}
                    onChange={(event) =>
                      setEditDialog({ ...editDialog, name: event.target.value })
                    }
                  />
                </label>
                <label>
                  <span>Description</span>
                  <textarea
                    className="field"
                    rows={4}
                    maxLength={300}
                    placeholder="Add an optional description"
                    value={editDialog.description}
                    onChange={(event) =>
                      setEditDialog({
                        ...editDialog,
                        description: event.target.value,
                      })
                    }
                  />
                </label>
              </div>
            </div>
            {editDialog.cover && (
              <p className="edit-playlist-legal">
                By saving, you confirm you have the right to use this image.
              </p>
            )}
            <div className="inline-actions">
              <button
                type="button"
                className="pill"
                onClick={() => setEditDialog(null)}
              >
                Cancel
              </button>
              <button
                type="submit"
                className="pill primary"
                disabled={!editDialog.name.trim()}
              >
                Save
              </button>
            </div>
          </form>
        </div>
      )}
      {removeDialog && (
        <div className="modal-backdrop" onClick={() => setRemoveDialog(null)}>
          <section
            className="dialog"
            role="alertdialog"
            aria-labelledby="remove-playlist-title"
            onClick={(event) => event.stopPropagation()}
          >
            <h2 id="remove-playlist-title">
              {removeDialog.own
                ? "Delete from Your Library?"
                : "Remove from Your Library?"}
            </h2>
            <p>
              {removeDialog.own
                ? `This will delete ${removeDialog.name} from Your Library.`
                : `${removeDialog.name} will be removed from Your Library.`}
            </p>
            <div className="inline-actions">
              <button
                className="pill"
                autoFocus
                onClick={() => setRemoveDialog(null)}
              >
                Cancel
              </button>
              <button
                className="pill primary"
                onClick={() => {
                  void call("remove_playlist", {
                    id: removeDialog.id,
                    name: removeDialog.name,
                  });
                  updateSettings(
                    saveAccountOrder(
                      state,
                      forgetPlaylist(accountOrder(state), removeDialog.id),
                    ),
                  );
                  if (state.route === `playlist:${removeDialog.id}`)
                    navigate("home");
                  setRemoveDialog(null);
                }}
              >
                {removeDialog.own ? "Delete" : "Remove"}
              </button>
            </div>
          </section>
        </div>
      )}
      {folderDialog && (
        <div className="modal-backdrop" onClick={() => setFolderDialog(false)}>
          <section
            className="dialog"
            onClick={(event) => event.stopPropagation()}
          >
            <h2>Create a folder</h2>
            <p>Adds a folder to Your Library on Spotify.</p>
            <input
              className="field"
              placeholder="Folder name"
              value={folderName}
              onChange={(event) => setFolderName(event.target.value)}
              autoFocus
            />
            <div className="inline-actions">
              <button className="pill" onClick={() => setFolderDialog(false)}>
                Cancel
              </button>
              <button
                className="pill primary"
                disabled={!folderName.trim()}
                onClick={() => {
                  void call("create_folder", { name: folderName.trim() });
                  setFolderName("");
                  setFolderDialog(false);
                }}
              >
                Create folder
              </button>
            </div>
          </section>
        </div>
      )}
      {sessionScreen(state) === "restoring" && (
        <div
          className="startup-screen"
          role="status"
          aria-label="Restoring session"
        >
          <MicLoader size={56} />
          <strong>Lyssna</strong>
          <span>Restoring session…</span>
        </div>
      )}
      {sessionScreen(state) === "setup" && (
        <Setup
          state={state}
          onSignIn={(clientId) => {
            dispatch({ type: "login_pending" });
            void rpc("sign_in", { clientId }).catch((error) =>
              dispatch({
                type: "login_failed",
                issue: { message: String(error), retry_saved: false },
              }),
            );
          }}
          onRetry={() => {
            dispatch({ type: "login_pending", restoring: true });
            void rpc("retry_sign_in").catch((error) =>
              dispatch({
                type: "login_failed",
                issue: { message: String(error), retry_saved: true },
              }),
            );
          }}
          onCancel={() => void call("cancel_sign_in")}
          onOpenDashboard={() =>
            void call("open_link", {
              url: "https://developer.spotify.com/dashboard",
            })
          }
        />
      )}
      <UpdatePrompt
        state={state}
        enabled={sessionScreen(state) !== "restoring"}
        current={nightlyCommit}
      />
      {firstBuildOpen && switchingToNightly && (
        <NightlyDialog
          report={state.nightly ?? null}
          starting={false}
          onStart={() => void call("nightly_build")}
          onCancelBuild={() => void call("nightly_cancel")}
          onHide={() => setFirstBuildOpen(false)}
          onLog={() => void call("nightly_log")}
        />
      )}
      {state.toast && (
        <div
          className={`toast ${state.toast.error ? "error" : ""}`}
          role="status"
        >
          <span>{state.toast.message}</span>
          <button
            className="icon-button"
            aria-label="Dismiss notification"
            onClick={() => dispatch({ type: "toast_clear" })}
          >
            <X size={16} />
          </button>
        </div>
      )}
      <input
        className="file-input"
        type="file"
        accept=".ttml,.xml,.lrc"
        ref={fileInput}
        onChange={(event) => {
          const file = event.target.files?.[0];
          if (file) void importFile(file);
          event.target.value = "";
        }}
      />
    </div>
  );
}

/** Where the pointer sits along a slider, 0 to 1. Sets `--hover` for the
 * light preview fill Spotify shows ahead of the current value. */
function hoverPreview(input: HTMLInputElement, clientX: number) {
  const box = input.getBoundingClientRect();
  const at = Math.min(1, Math.max(0, (clientX - box.left) / box.width));
  input.style.setProperty("--hover", `${at * 100}%`);
  return at;
}
/** The player bar's song time and seek bar: the only part that follows
 *  song time every frame, so the rest of the bar does not render again. */
function SeekRow({
  state,
  onPlayer,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
}) {
  const current = usePosition(state),
    track = state.playback.item;
  const [seeking, setSeeking] = useState<number | null>(null),
    [seekHover, setSeekHover] = useState<number | null>(null);
  const seek = () => {
    if (seeking !== null) {
      onPlayer({ command: "seek", positionMs: seeking });
      setSeeking(null);
    }
  };
  useEffect(() => setSeeking(null), [track?.uri]);
  return (
    <div className="seek-row">
      <time>{formatTime(seeking ?? current)}</time>
      <div className="seek-track">
        {seekHover !== null && track?.duration_ms ? (
          <span
            className="seek-tooltip"
            style={{ left: `${seekHover * 100}%` }}
            aria-hidden="true"
          >
            {formatTime(seekHover * track.duration_ms)}
          </span>
        ) : null}
        <input
          aria-label="Seek position"
          type="range"
          min="0"
          max={track?.duration_ms || 1}
          value={seeking ?? current}
          style={
            {
              "--range": `${((seeking ?? current) / Math.max(1, track?.duration_ms || 1)) * 100}%`,
            } as CSSProperties
          }
          onChange={(event) => setSeeking(Number(event.target.value))}
          onPointerDown={(event) =>
            event.currentTarget.setPointerCapture(event.pointerId)
          }
          onPointerUp={seek}
          onKeyUp={seek}
          onPointerCancel={() => setSeeking(null)}
          onLostPointerCapture={() => setSeeking(null)}
          onBlur={seek}
          onPointerMove={(event) =>
            setSeekHover(hoverPreview(event.currentTarget, event.clientX))
          }
          onPointerLeave={(event) => {
            event.currentTarget.style.removeProperty("--hover");
            setSeekHover(null);
          }}
        />
      </div>
      <time>{formatTime(track?.duration_ms || 0)}</time>
    </div>
  );
}

function PlayerBar({
  state,
  onPlayer,
  onDJRequest,
  onToggle,
  onAlbum,
  onArtist,
  onSave,
  onLyrics,
  onQueue,
  onDevices,
  onSidebar,
  onFullscreen,
  lyricsActive,
  sidebarActive,
  queueActive,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  onDJRequest: () => void;
  onToggle: () => void;
  onAlbum: (album: Album) => void;
  onArtist: (artist: Artist) => void;
  onSave: (track: Track) => void;
  onLyrics: () => void;
  onQueue: () => void;
  onDevices: () => void;
  onSidebar: () => void;
  onFullscreen: () => void;
  lyricsActive: boolean;
  sidebarActive: boolean;
  queueActive: boolean;
}) {
  const track = state.playback.item;
  const [volume, setVolume] = useState(70);
  const volumeCallback = useRef(onPlayer);
  volumeCallback.current = onPlayer;
  const volumeControl = useMemo(
    () =>
      new VolumeControl(
        (value) => volumeCallback.current({ command: "volume", volume: value }),
        setVolume,
      ),
    [],
  );
  useEffect(() => () => volumeControl.dispose(), [volumeControl]);
  const saved = track && state.saved[track.uri];
  const elsewhere =
    !!state.playback.device.name &&
    state.playback.device.is_active &&
    state.playback.device.name !== state.local_device_name;
  useEffect(() => {
    volumeControl.observe(
      state.playback.device.volume_percent,
      state.playback.device.id ?? state.playback.device.name,
    );
  }, [
    state.playback.device.volume_percent,
    state.playback.device.id,
    state.playback.device.name,
    volumeControl,
  ]);
  return (
    <footer
      className={`player-bar ${elsewhere ? "has-elsewhere" : ""}`}
      inert={!state.profile}
    >
      <div className="player-song">
        {state.narrating ? (
          <DJPresentation playing={state.playback.is_playing} />
        ) : track ? (
          <>
            <Cover
              images={track.album.images}
              contextRoute={
                track.album.id
                  ? `album:${track.album.id}`
                  : track.album.uri?.replace("spotify:", "")
              }
              contextName={track.album.name}
              onClick={() => onAlbum(track.album)}
            />
            <div>
              <TrackTitle
                track={track}
                onAlbum={onAlbum}
                reduceMotion={state.settings.reduce_motion}
              />
              <Marquee
                text={artistNames(track)}
                reduceMotion={state.settings.reduce_motion}
              >
                <ExplicitBadge track={track} />
                <ArtistLinks artists={track.artists} onArtist={onArtist} />
              </Marquee>
            </div>
            <button
              className={`icon-button ${saved ? "is-saved" : ""}`}
              title="Save to Liked Songs"
              onClick={() => onSave(track)}
            >
              {saved ? (
                <CircleCheck size={20} fill="currentColor" stroke="#000" />
              ) : (
                <CirclePlus size={20} />
              )}
            </button>
          </>
        ) : (
          <div className="player-placeholder">
            Make room for your next favourite.
          </div>
        )}
      </div>
      <div className="player-transport">
        <div className="transport-buttons">
          <button
            className={`icon-button ${state.playback.shuffle_state ? "active" : ""}`}
            title="Shuffle"
            onClick={() =>
              onPlayer({
                command: "shuffle",
                enabled: !state.playback.shuffle_state,
              })
            }
          >
            <ShuffleIcon size={18} />
          </button>
          <button
            className="icon-button"
            title="Previous song"
            onClick={() => onPlayer({ command: "previous" })}
          >
            <PreviousIcon size={16.5} />
          </button>
          <button
            className="play-toggle"
            aria-label={state.playback.is_playing ? "Pause" : "Play"}
            title="Play / pause · Space"
            onClick={onToggle}
            disabled={!track}
          >
            {state.pending ? (
              <LoaderCircle className="spin" size={18} />
            ) : state.playback.is_playing ? (
              <PauseIcon size={18} />
            ) : (
              <PlayIcon size={18} />
            )}
          </button>
          <button
            className="icon-button"
            title="Next song"
            onClick={() => onPlayer({ command: "next" })}
          >
            <NextIcon size={16.5} />
          </button>
          <button
            className={`icon-button ${state.playback.repeat_state !== "off" && state.playback.repeat_state ? "active" : ""}`}
            title="Repeat"
            onClick={() =>
              onPlayer({
                command: "repeat",
                repeat:
                  state.playback.repeat_state === "off" ||
                  !state.playback.repeat_state
                    ? "context"
                    : state.playback.repeat_state === "context"
                      ? "track"
                      : "off",
              })
            }
          >
            {state.playback.repeat_state === "track" ? (
              <RepeatIcon one size={18} />
            ) : (
              <RepeatIcon size={18} />
            )}
          </button>
        </div>
        <SeekRow state={state} onPlayer={onPlayer} />
      </div>
      <div className="player-extras">
        {(state.dj_context || state.narrating || state.dj_pending) && (
          <DJButton
            onRefresh={() => onPlayer({ command: "dj" })}
            onHold={onDJRequest}
          />
        )}
        <button
          className={`icon-button ${lyricsActive ? "active" : ""}`}
          title="Lyrics"
          onClick={onLyrics}
        >
          <Mic2 size={20} />
        </button>
        <button
          className={`icon-button ${queueActive ? "active" : ""}`}
          title="Queue"
          onClick={onQueue}
        >
          <ListMusic size={21} />
        </button>
        <button className="icon-button" title="Devices" onClick={onDevices}>
          <Speaker size={20} />
        </button>
        <button
          className={`icon-button ${sidebarActive ? "active" : ""}`}
          title="Now playing"
          onClick={onSidebar}
        >
          <PanelRight size={20} />
        </button>
        <button
          className="icon-button volume-button"
          title={volume ? "Mute" : "Unmute"}
          onClick={() => {
            const next = volume ? 0 : 70;
            volumeControl.edit(next);
            volumeControl.finish();
          }}
        >
          {volume ? <Volume2 size={21} /> : <VolumeX size={21} />}
        </button>
        <input
          className="volume-range"
          aria-label="Volume"
          type="range"
          min="0"
          max="100"
          value={volume}
          onPointerDown={() => volumeControl.begin()}
          onPointerUp={() => volumeControl.finish()}
          onPointerCancel={() => volumeControl.finish()}
          onKeyUp={() => volumeControl.finish()}
          onBlur={() => volumeControl.finish()}
          style={{ "--range": `${volume}%` } as CSSProperties}
          onPointerMove={(event) =>
            hoverPreview(event.currentTarget, event.clientX)
          }
          onPointerLeave={(event) =>
            event.currentTarget.style.removeProperty("--hover")
          }
          onChange={(event) => {
            const next = Number(event.target.value);
            volumeControl.edit(next);
          }}
        />
        {state.settings.cinema_lyrics && (
          <button
            className="icon-button"
            title="Cinema Lyrics"
            aria-label="Cinema Lyrics"
            onClick={() => void rpc("toggle_cinema_lyrics").catch(() => {})}
          >
            <CinemaIcon />
          </button>
        )}
        <button
          className="icon-button"
          title="Lyrics popout"
          aria-label="Lyrics popout"
          onClick={() => void rpc("open_lyrics_popout").catch(() => {})}
        >
          <PopoutIcon />
        </button>
        <button
          className="icon-button"
          title="Full screen lyrics"
          onClick={onFullscreen}
        >
          <Maximize2 size={19} />
        </button>
      </div>
      {/* Like Spotify, a green bar names the speaker playing when it is
          not this computer, and opens the device list. */}
      {elsewhere && (
        <button
          className="playing-elsewhere"
          onClick={onDevices}
          title="Connect to a device"
        >
          <Speaker size={14} />
          Playing on {state.playback.device.name}
        </button>
      )}
    </footer>
  );
}

function Setup({
  state,
  onSignIn,
  onRetry,
  onCancel,
  onOpenDashboard,
}: {
  state: AppState;
  onSignIn: (id: string) => void;
  onRetry: () => void;
  onCancel: () => void;
  onOpenDashboard: () => void;
}) {
  const [id, setId] = useState(state.settings.client_id);
  const [developerMode, setDeveloperMode] = useState(false);
  useEffect(() => {
    setId(state.settings.client_id);
  }, [state.settings.client_id]);
  return (
    <div className="setup-backdrop">
      <SetupAmbience
        still={
          state.settings.reduce_motion ||
          window.matchMedia("(prefers-reduced-motion: reduce)").matches
        }
      />
      <section
        className="setup-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="setup-title"
        onKeyDown={(event) => {
          if (event.key === "Escape") event.stopPropagation();
        }}
      >
        <div className="setup-logo">
          <Logo size={40} />
        </div>
        <h1 id="setup-title">
          {state.login_issue ? "Reconnect Spotify" : "Log in to Lyssna"}
        </h1>
        <p>Use your Spotify account to play your music.</p>
        <p className="setup-premium">Spotify Premium required</p>
        {state.login_issue && (
          <div className="setup-recovery" role="alert">
            <p>{state.login_issue.message}</p>
            {state.login_issue.retry_saved &&
              id === state.settings.client_id && (
                <button
                  className="pill"
                  disabled={state.login_pending}
                  onClick={onRetry}
                >
                  Retry saved sign-in
                </button>
              )}
          </div>
        )}
        {developerMode && (
          <div className="setup-steps">
            <button className="text-link" onClick={onOpenDashboard}>
              1. Create an app in Spotify Developer Dashboard{" "}
              <ExternalLink size={14} />
            </button>
            <label>
              2. Add this redirect URI
              <code>http://127.0.0.1:43821/callback</code>
            </label>
            <label htmlFor="client-id">3. Enter your Client ID</label>
            <input
              className="field"
              id="client-id"
              value={id}
              onChange={(event) => setId(event.target.value.trim())}
              placeholder="32-character Spotify Client ID"
              autoComplete="off"
              spellCheck={false}
              disabled={state.login_pending}
            />
          </div>
        )}
        <button
          className="pill primary setup-sign-in"
          disabled={
            state.login_pending ||
            !state.booted ||
            (developerMode && !/^[a-fA-F0-9]{32}$/.test(id))
          }
          onClick={() => onSignIn(developerMode ? id : "")}
        >
          {state.login_pending || !state.booted ? (
            <>
              <LoaderCircle size={18} className="spin" />
              Connecting…
            </>
          ) : state.login_issue ? (
            "Sign in again"
          ) : (
            "Sign in with Spotify"
          )}
        </button>
        {state.login_pending ? (
          <>
            <p className="setup-help">
              Complete sign-in in your browser.
              {developerMode
                ? " Audio playback may request another authorization."
                : " Return here when Spotify finishes signing you in."}
            </p>
            <button className="text-link muted" onClick={onCancel}>
              Cancel sign-in
            </button>
          </>
        ) : (
          <button
            className="text-link muted setup-mode"
            aria-expanded={developerMode}
            onClick={() => setDeveloperMode(!developerMode)}
          >
            {developerMode
              ? "Use Spotify account sign-in"
              : "Developer connection (optional)"}
          </button>
        )}
        <small>No password or client secret is stored in Lyssna.</small>
      </section>
    </div>
  );
}

export default App;
