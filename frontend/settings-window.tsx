import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import {
  AppWindow,
  FileText,
  FlaskConical,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  ChevronDown,
  ExternalLink,
  LibraryBig,
  LogOut,
  Search,
  Settings2,
  Palette,
  X,
  AlignLeft,
  Wrench,
} from "lucide-react";
import type { AppState } from "./state";
import type { NightlyReport, Settings, SpotifyPlus } from "./types";
import { NightlyDialog, nightlyProgress } from "./nightly-dialog";
import {
  HiddenSettings,
  SettingRow,
  SettingsQuery,
  Toggle,
} from "./setting-row";
import { cloneElement, isValidElement, type ReactElement } from "react";
import { IS_MAC } from "./platform";
import { CREDITS, Credits } from "./credits";
import { FontSettings } from "./custom-fonts";
import { DEFAULT_MARKER, MARKER_COLOURS } from "./highlighter";
import { LyricsProvider } from "./lyrics-provider";
import { MusixmatchToken } from "./musixmatch-token";
import { AnimatorPresetRow } from "./animator-settings";
import { SOLOIST_PLAYS, SoloistRows } from "./soloist-settings";

/** Default source order; LRCLIB and Netease start off. */
const DEFAULT_SOURCES = [
  "spicy",
  "apple",
  "musixmatch",
  "spotify",
  "lrclib",
  "netease",
];
const DEFAULT_SOURCES_OFF = ["lrclib", "netease"];
/** Lyric sources Lyssna can use, as listed in Sources. */
const SOURCES: Record<string, { name: string; help: string }> = {
  spicy: {
    name: "Spicy Lyrics",
    help: "Community-contributed lyrics from Spicy Lyrics. Receives the track ID and your IP address.",
  },
  musixmatch: {
    name: "Musixmatch",
    help: "Synced or word-timed lyrics through Musixmatch's desktop-app service (unofficial). Receives the title, artists, album, length and track ID.",
  },
  apple: {
    name: "Apple Music",
    help: "Apple Music lyrics through the Spicy Lyrics provider.",
  },
  spotify: {
    name: "Spotify",
    help: "Spotify's own lyrics, through your signed-in session.",
  },
  lrclib: {
    name: "LRCLIB",
    help: "Free open lyrics database. Receives the title, artist, album and length.",
  },
  netease: {
    name: "Netease",
    help: "Netease community lyrics through a third-party proxy. Receives the title and artist.",
  },
};

const TABS = [
  ["General", Settings2],
  ["Appearance", Palette],
  ["Library", LibraryBig],
  ["Lyrics", AlignLeft],
  ["Effects", FlaskConical],
  ["Layout", AppWindow],
  ["Sources", FileText],
  ["Advanced", Wrench],
] as const;
/** About opens from the footer card rather than the tab bar. */
export type SettingsTab = (typeof TABS)[number][0] | "About";
type Tab = SettingsTab;

/** Settings reopen on the tab and scroll position they were left at. */
const PLACE_KEY = "lyssna.settings-place";
function readPlace(): { tab: Tab; scroll: number } {
  try {
    const place = JSON.parse(localStorage.getItem(PLACE_KEY) ?? "null");
    if (
      place &&
      [...TABS.map(([label]) => label), "About"].includes(place.tab) &&
      Number.isFinite(place.scroll)
    )
      return { tab: place.tab, scroll: Math.max(0, place.scroll) };
  } catch {
    // Storage may be unavailable; start at the top of General.
  }
  return { tab: "General", scroll: 0 };
}
function writePlace(tab: Tab, scroll: number) {
  try {
    localStorage.setItem(PLACE_KEY, JSON.stringify({ tab, scroll }));
  } catch {
    // Remembering the place is a convenience only.
  }
}
/** The tab Settings was last left on, or General when it should not be
 *  remembered. */
export const lastSettingsTab = (remember = true): Tab =>
  remember ? readPlace().tab : "General";

/** Downloads and Music read as Spotify names them; other folders by path. */
function localSourceName(path: string) {
  const last = path.split(/[\\/]/).filter(Boolean).pop();
  return last === "Downloads"
    ? "Downloads"
    : last === "Music"
      ? "Music Library"
      : path;
}
type Flag = {
  [K in keyof Settings]-?: Settings[K] extends boolean | undefined ? K : never;
}[keyof Settings];
type Amount = {
  [K in keyof Settings]-?: Settings[K] extends number | undefined ? K : never;
}[keyof Settings];

/** Every Lyssna setting in one window, laid out like Spicy Lyrics'
 *  settings panel: category tabs, search across all of them, then rows
 *  grouped into cards. */
export function SettingsWindow({
  state,
  initialTab = "General",
  onSettings,
  onLoadTtml,
  onDatabase,
  onOpen,
  onClose,
  onCall,
  onNotice,
  onManageAnimator,
}: {
  initialTab?: Tab;
  /** Opens Custom animator tuning beside full screen lyrics. */
  onManageAnimator: () => void;
  state: AppState;
  onSettings: (settings: Partial<Settings>) => void;
  onCall: <T = void>(
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<T | undefined>;
  onNotice: (message: string, error?: boolean) => void;
  onLoadTtml: () => void;
  onDatabase: () => void;
  onOpen: (url: string) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<Tab>(initialTab);
  // Back on the tab it was left at, Settings scrolls to where it was.
  useLayoutEffect(() => {
    const place = readPlace();
    if (
      place.tab !== initialTab ||
      state.settings.settings_remember_place === false
    ) {
      writePlace(initialTab, 0);
      return;
    }
    // Rows fill in over the first frames (audio outputs, fonts); keep
    // restoring until the page is tall enough or a moment has passed.
    const node = scroll.current;
    const opened = performance.now();
    let frame = 0;
    const restore = () => {
      node?.scrollTo({ top: place.scroll });
      if (
        node &&
        node.scrollHeight - node.clientHeight < place.scroll &&
        performance.now() - opened < 600
      )
        frame = requestAnimationFrame(restore);
    };
    restore();
    return () => cancelAnimationFrame(frame);
    // Only when the window opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [query, setQuery] = useState("");
  const scroll = useRef<HTMLDivElement>(null);
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  // The highlight slides under the selected tab; hidden while searching.
  const [marker, setMarker] = useState<{ left: number; width: number }>();
  const searching = !!query.trim();
  // Closing records the place too: a scroll right before closing may not
  // have reported yet.
  const place = useRef({ tab, searching });
  place.current = { tab, searching };
  useLayoutEffect(() => {
    const node = scroll.current;
    return () => {
      if (node && !place.current.searching)
        writePlace(place.current.tab, node.scrollTop);
    };
  }, []);
  const settings = state.settings;
  const soloistPlays = settings.soloist ? SOLOIST_PLAYS : undefined;
  const [audioOutputs, setAudioOutputs] = useState<string[] | null>(null);
  const [audioRefresh, setAudioRefresh] = useState(0);
  const [testingOutput, setTestingOutput] = useState(false);
  const [cacheBytes, setCacheBytes] = useState<number | null>(null);
  // Nightly is built on this computer; the dialog shows its progress.
  const [nightlyOpen, setNightlyOpen] = useState(false);
  const [nightlyStarting, setNightlyStarting] = useState(false);
  const [nightlyStatus, setNightlyStatus] = useState<{
    commit: string | null;
    report: NightlyReport | null;
  }>();
  useEffect(() => {
    void onCall<{ commit: string | null; report: NightlyReport | null }>(
      "nightly_status",
    ).then((status) => status && setNightlyStatus(status));
  }, [onCall]);
  const nightly = state.settings.update_channel === "nightly";
  const nightlyReport = state.nightly ?? nightlyStatus?.report ?? null;
  const nightlyBuilding = nightlyStarting || !!nightlyReport?.running;
  // The channel turns Nightly once the build installs; show the choice
  // from the moment the build starts.
  const nightlyChosen = nightly || nightlyBuilding;
  useEffect(() => {
    if (state.nightly) setNightlyStarting(false);
  }, [state.nightly]);
  const buildNightly = () => {
    setNightlyStarting(true);
    setNightlyOpen(true);
    void onCall<boolean>("nightly_build").then((started) => {
      if (!started) setNightlyStarting(false);
    });
  };
  // Read from Windows each time Settings opens: Task Manager can change it.
  const [launch, setLaunch] = useState<"off" | "open" | "minimized" | null>(
    null,
  );
  useEffect(() => {
    let active = true;
    void onCall<"off" | "open" | "minimized">("autostart").then((value) => {
      if (active && value) setLaunch(value);
    });
    return () => {
      active = false;
    };
  }, [onCall]);
  const [clearingCache, setClearingCache] = useState(false);
  const [clearingNightly, setClearingNightly] = useState(false);
  useEffect(() => {
    let active = true;
    void onCall<number>("audio_cache_size").then((bytes) => {
      if (active && typeof bytes === "number") setCacheBytes(bytes);
    });
    return () => {
      active = false;
    };
  }, [onCall]);
  useEffect(() => {
    if (!state.profile?.id) return;
    let active = true;
    void onCall<string[]>("audio_outputs").then((outputs) => {
      if (active && Array.isArray(outputs)) setAudioOutputs(outputs);
    });
    return () => {
      active = false;
    };
  }, [onCall, state.profile?.id, audioRefresh]);
  const plus = settings.spotify_plus ?? {};
  const setPlus = (change: Partial<SpotifyPlus>) =>
    onSettings({ spotify_plus: { ...plus, ...change } });
  const chosenFolders = new Set(plus.playlist_folders ?? []);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose]);
  useLayoutEffect(() => {
    const index = TABS.findIndex(([label]) => label === tab);
    const measure = () => {
      const button = tabs.current[index];
      if (button)
        setMarker({ left: button.offsetLeft, width: button.offsetWidth });
    };
    measure();
    const observer = new ResizeObserver(measure);
    const list = tabs.current[index]?.parentElement;
    if (list) observer.observe(list);
    return () => observer.disconnect();
  }, [tab]);
  const select = (next: Tab) => {
    setTab(next);
    setQuery("");
    scroll.current?.scrollTo({ top: 0 });
    writePlace(next, 0);
  };
  const flag = (
    key: Flag,
    label: string,
    help: string,
    keywords = "",
    unavailable?: string,
  ) => (
    <SettingRow
      key={key}
      label={label}
      help={help}
      keywords={keywords}
      unavailable={unavailable}
    >
      <Toggle
        label={label}
        checked={!!settings[key]}
        onChange={(checked) => onSettings({ [key]: checked })}
      />
    </SettingRow>
  );
  /** A slider row; `off` dims it while the option it tunes is turned off. */
  const range = (
    key: Amount,
    label: string,
    help: string,
    keywords: string,
    [min, max, step]: [number, number, number],
    fallback: number,
    unit = "",
    off = false,
  ) => {
    const value = settings[key] ?? fallback;
    return (
      <SettingRow key={key} label={label} help={help} keywords={keywords}>
        <span className={`settings-range ${off ? "off" : ""}`}>
          <input
            type="range"
            aria-label={label}
            min={min}
            max={max}
            step={step}
            value={value}
            disabled={off}
            style={
              {
                "--fill": `${Math.min(100, Math.max(0, ((value - min) / (max - min)) * 100))}%`,
              } as CSSProperties
            }
            onChange={(event) =>
              onSettings({ [key]: Number(event.target.value) })
            }
          />
          <output>
            {value}
            {unit}
          </output>
        </span>
      </SettingRow>
    );
  };
  const lyricSize = settings.lyric_size || 36;
  const order = settings.lyrics_source_order?.length
    ? settings.lyrics_source_order
    : DEFAULT_SOURCES;
  const disabled = new Set(
    settings.lyrics_sources_disabled ?? DEFAULT_SOURCES_OFF,
  );
  const move = (id: string, step: -1 | 1) => {
    const next = [...order];
    const index = next.indexOf(id);
    const to = index + step;
    if (index < 0 || to < 0 || to >= next.length) return;
    [next[index], next[to]] = [next[to], next[index]];
    onSettings({ lyrics_source_order: next });
  };
  const sections: [Tab, string, React.ReactNode[]][] = [
    [
      "General",
      "Account",
      [
        state.login_issue && !searching ? (
          <div key="login" className="settings-alert" role="alert">
            <p>{state.login_issue.message}</p>
            <button
              className="pill"
              disabled={state.login_pending}
              onClick={() => void onCall("sign_in", { clientId: "" })}
            >
              Sign in again
            </button>
          </div>
        ) : null,
        <SettingRow
          key="account"
          label={state.profile?.display_name || state.profile?.id || "Spotify"}
          help="Spotify connected"
          keywords="account sign out log out profile"
        >
          <button
            className="pill outline"
            onClick={() => void onCall("sign_out")}
          >
            <LogOut size={14} />
            Sign out
          </button>
        </SettingRow>,
        !state.can_follow ? (
          <SettingRow
            key="follow"
            label="Artist following"
            help="Allow Lyssna to follow your favourite artists."
            keywords="permissions scopes"
          >
            <button
              className="pill outline"
              onClick={() => void onCall("sign_in", { clientId: "" })}
            >
              Update permissions
              <ExternalLink size={13} />
            </button>
          </SettingRow>
        ) : null,
      ],
    ],
    [
      "General",
      "Playback",
      [
        <SoloistRows
          key="soloist"
          settings={settings}
          status={state.soloist}
          keysRefused={!!state.keys_refused}
          onSettings={onSettings}
          onCall={onCall}
          onOpen={onOpen}
          onNotice={onNotice}
        />,
        <SettingRow
          key="output"
          label="Audio output"
          help="Speakers or headphones for this computer."
          keywords="device speaker headphones sound"
          unavailable={soloistPlays}
        >
          <select
            className="settings-select"
            aria-label="Audio output"
            value={settings.audio_output || ""}
            onChange={(event) =>
              onSettings({ audio_output: event.target.value || null })
            }
          >
            <option value="">System default</option>
            {settings.audio_output &&
              !audioOutputs?.includes(settings.audio_output) && (
                <option value={settings.audio_output}>
                  {settings.audio_output}
                  {audioOutputs ? " (unavailable)" : ""}
                </option>
              )}
            {audioOutputs?.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
        </SettingRow>,
        flag(
          "crossfade",
          "Crossfade songs",
          "Fade each song into the next when one runs into the other, as Spotify does. Songs you pick start straight away.",
          "crossfade fade transition gapless overlap blend",
          soloistPlays,
        ),
        settings.crossfade ? (
          <SettingRow
            key="crossfade-seconds"
            label={`Crossfade length: ${settings.crossfade_seconds || 5} s`}
            help="How long the end of one song overlaps the start of the next."
            keywords="crossfade seconds length duration"
            unavailable={soloistPlays}
          >
            <span className="settings-range">
              <input
                type="range"
                aria-label="Crossfade seconds"
                min="1"
                max="12"
                value={settings.crossfade_seconds || 5}
                style={
                  {
                    "--fill": `${(((settings.crossfade_seconds || 5) - 1) / 11) * 100}%`,
                  } as CSSProperties
                }
                onChange={(event) =>
                  onSettings({ crossfade_seconds: Number(event.target.value) })
                }
              />
            </span>
          </SettingRow>
        ) : null,
        <SettingRow
          key="refresh"
          label="New headphones or speakers"
          help="Find outputs you connected while Lyssna was open."
          keywords="audio output device refresh"
          unavailable={soloistPlays}
        >
          <button
            className="pill outline"
            onClick={() => setAudioRefresh((value) => value + 1)}
          >
            Refresh outputs
          </button>
        </SettingRow>,
        <SettingRow
          key="test"
          label="Check output"
          help="Play two quiet tones on the selected output."
          keywords="audio output test sound"
          unavailable={soloistPlays}
        >
          <button
            className="pill outline"
            disabled={testingOutput}
            onClick={async () => {
              setTestingOutput(true);
              try {
                if (await onCall<boolean>("test_audio_output"))
                  onNotice("Test sound sent to the selected output");
              } finally {
                setTestingOutput(false);
              }
            }}
          >
            {testingOutput ? "Playing test sound…" : "Test output"}
          </button>
        </SettingRow>,
        <SettingRow
          key="native"
          label="Native audio"
          help={
            <span className={state.native_ready ? "native-ready" : ""}>
              {state.native_status}
            </span>
          }
          keywords="native audio reconnect playback engine"
          unavailable={soloistPlays}
        >
          <button
            className="pill outline"
            onClick={() => void onCall("retry_audio")}
          >
            Reconnect native audio
          </button>
        </SettingRow>,
      ],
    ],
    [
      "General",
      "Startup and window behaviour",
      [
        <SettingRow
          key="autostart"
          label="Open Lyssna automatically after you log into the computer"
          keywords="startup autostart login sign in boot launch minimized"
        >
          <select
            className="settings-select"
            aria-label="Open Lyssna automatically"
            value={launch ?? "off"}
            disabled={launch === null}
            onChange={(event) => {
              const next = event.target.value as "off" | "open" | "minimized";
              setLaunch(next);
              void onCall("set_autostart", { launch: next });
            }}
          >
            <option value="off">No</option>
            <option value="open">Yes</option>
            <option value="minimized">Minimized</option>
          </select>
        </SettingRow>,
        flag(
          "keep_playing_on_close",
          "Close button should minimize the Lyssna window",
          "",
          "close minimize minimise tray background notification area system tray quit keep playing",
        ),
      ],
    ],
    [
      "General",
      "Storage",
      [
        <SettingRow
          key="cache"
          label={`Cache: ${cacheBytes === null ? "…" : `${Math.round(cacheBytes / 1_048_576).toLocaleString()} MB`}`}
          help="Songs you have played, kept so they start faster and use less data."
          keywords="cache storage disk offline data clear"
        >
          <button
            className="pill outline"
            disabled={clearingCache || !cacheBytes}
            onClick={() => {
              setClearingCache(true);
              void onCall<number>("clear_audio_cache")
                .then((freed) => {
                  setCacheBytes(0);
                  onNotice(
                    `Cleared ${Math.round((freed ?? 0) / 1_048_576).toLocaleString()} MB of cached songs.`,
                  );
                })
                .finally(() => setClearingCache(false));
            }}
          >
            Clear cache
          </button>
        </SettingRow>,
        <SettingRow
          key="lyrics-cache"
          label="Lyrics cache"
          help="Saved lyrics are reused for three days. Clear them to fetch newly uploaded lyrics for every song. Imported lyrics are kept."
          keywords="lyrics cache clear refresh upload spicy stale"
        >
          <button
            className="pill outline"
            onClick={() =>
              void onCall("clear_lyrics_cache").then(() =>
                onNotice("Cleared saved lyrics."),
              )
            }
          >
            Clear lyrics
          </button>
        </SettingRow>,
      ],
    ],
    [
      "General",
      "Updates",
      [
        <SettingRow
          key="version"
          label={`Version ${__APP_VERSION__}${nightlyStatus?.commit ? ` · Nightly ${nightlyStatus.commit.slice(0, 7)}` : ""}`}
          help={
            state.update_version
              ? `${state.update_status} · v${state.update_version}`
              : state.update_status
          }
          keywords="updates version check install"
        >
          <button
            className="pill outline"
            disabled={
              (IS_MAC && !nightly) ||
              state.update_status.startsWith("Downloading update") ||
              state.update_status.startsWith("Building update")
            }
            onClick={() =>
              void onCall("app_update", {
                action:
                  state.update_status === "Ready to install"
                    ? "install"
                    : state.update_status === "Update available"
                      ? "download"
                      : "check",
              })
            }
          >
            {state.update_status === "Ready to install"
              ? "Install update and restart"
              : state.update_status === "Update available"
                ? "Download update"
                : "Check for updates"}
          </button>
        </SettingRow>,
        !IS_MAC || nightly ? (
          <SettingRow
            key="auto-update"
            label={
              nightly
                ? "Build Nightly updates automatically"
                : "Download updates automatically"
            }
            help={
              nightly
                ? "New code is built in the background; you choose when to restart."
                : "New versions are verified before they install."
            }
            keywords="updates auto nightly build"
          >
            <Toggle
              label={
                nightly
                  ? "Build Nightly updates automatically"
                  : "Download updates automatically"
              }
              checked={settings.auto_download_updates}
              onChange={(auto_download_updates) =>
                onSettings({ auto_download_updates })
              }
            />
          </SettingRow>
        ) : null,
      ],
    ],
    [
      "Appearance",
      "Theme",
      [
        flag(
          "liquid_glass",
          "Liquid Glass",
          "Frosted, translucent panels that pick up colour from the artwork you're playing.",
          "theme glass blur translucent apple transparency",
        ),
        flag(
          "reduce_motion",
          "Reduce motion",
          "Turn off animated backgrounds and word animations.",
          "animation accessibility",
        ),
        flag(
          "animation_fps_cap_enabled",
          "Limit animation frame rate",
          "Cap how often lyrics, scrolling and the moving background redraw, to save power.",
          "fps frame rate limit cap animation power battery",
        ),
        range(
          "animation_fps_cap",
          "Animation frame rate",
          "The most frames per second animations draw while the limit is on.",
          "fps frame rate limit cap animation",
          [15, 240, 5],
          60,
          " fps",
          !settings.animation_fps_cap_enabled,
        ),
      ],
    ],
    [
      "Appearance",
      "Font",
      [
        <FontSettings
          key="ui-font"
          field="ui_font"
          settings={settings}
          onSettings={onSettings}
          onNotice={onNotice}
        />,
      ],
    ],
    [
      "Library",
      "Your Library",
      [
        flag(
          "compact_library",
          "Compact library",
          "Show Your Library as a narrow column of covers.",
          "compact library sidebar",
        ),
        flag(
          "show_local_files",
          "Show Local Files",
          "Songs on this computer appear in Your Library as Local Files.",
          "local files mp3 flac folder computer",
        ),
        ...(settings.show_local_files
          ? (settings.local_sources ?? []).map((source, index) => {
              const name = localSourceName(source.path);
              const sources = settings.local_sources ?? [];
              return (
                <SettingRow
                  key={`local-${source.path}`}
                  label={name}
                  help={
                    index === 0 ? "Show songs from these folders." : undefined
                  }
                  keywords={`local files source folder ${source.path}`}
                >
                  <span className="settings-local-source">
                    {name === source.path ? (
                      <button
                        className="pill outline"
                        onClick={() =>
                          onSettings({
                            local_sources: sources.filter(
                              (item) => item.path !== source.path,
                            ),
                          })
                        }
                      >
                        Remove
                      </button>
                    ) : null}
                    <Toggle
                      label={name}
                      checked={source.enabled}
                      onChange={(enabled) =>
                        onSettings({
                          local_sources: sources.map((item) =>
                            item.path === source.path
                              ? { ...item, enabled }
                              : item,
                          ),
                        })
                      }
                    />
                  </span>
                </SettingRow>
              );
            })
          : []),
        settings.show_local_files ? (
          <SettingRow
            key="local-add"
            label="Add a source"
            help="Choose a folder of music. Songs with a .ttml or .lrc file of the same name get those lyrics."
            keywords="local files add source folder"
          >
            <button
              className="pill outline"
              onClick={async () => {
                const path = await onCall<string | null>("pick_local_folder");
                if (!path) return;
                const sources = settings.local_sources ?? [];
                onSettings({
                  local_sources: sources.some((item) => item.path === path)
                    ? sources.map((item) =>
                        item.path === path ? { ...item, enabled: true } : item,
                      )
                    : [...sources, { path, enabled: true }],
                });
              }}
            >
              Add a source
            </button>
          </SettingRow>
        ) : null,
        <SettingRow
          key="folders"
          stacked
          label="Add to playlist folders"
          help={
            state.library_folders?.length
              ? "Offer only playlists directly inside the folders you choose. Choose none to offer every playlist you can edit."
              : "Your library has no folders."
          }
          keywords="add playlist folders menu"
        >
          <div className="settings-folder-picks">
            {(state.library_folders ?? []).map((folder) => (
              <label key={folder.id}>
                <input
                  type="checkbox"
                  checked={chosenFolders.has(folder.id)}
                  onChange={(event) => {
                    const next = new Set(chosenFolders);
                    if (event.target.checked) next.add(folder.id);
                    else next.delete(folder.id);
                    setPlus({ playlist_folders: [...next] });
                  }}
                />
                <span>
                  {[
                    ...folder.parents.map(
                      (parent) =>
                        state.library_folders?.find(
                          (item) => item.id === parent,
                        )?.name ?? "",
                    ),
                    folder.name,
                  ]
                    .filter(Boolean)
                    .join(" / ")}
                </span>
              </label>
            ))}
          </div>
        </SettingRow>,
      ],
    ],
    [
      "Lyrics",
      "Look",
      [
        <SettingRow
          key="size"
          label="Lyric text size"
          help="Make room for every word."
          keywords="lyrics size font scale"
        >
          <span className="settings-range">
            <input
              type="range"
              aria-label="Lyric text size"
              min="18"
              max="60"
              value={lyricSize}
              style={
                {
                  "--fill": `${((lyricSize - 18) / 42) * 100}%`,
                } as CSSProperties
              }
              onChange={(event) =>
                onSettings({ lyric_size: Number(event.target.value) })
              }
            />
            <output>{Math.round(lyricSize)}</output>
          </span>
        </SettingRow>,
        <FontSettings
          key="font"
          field="lyric_font"
          settings={settings}
          onSettings={onSettings}
          onNotice={onNotice}
        />,
        <SettingRow
          key="background"
          label="Background"
          help="Moving artwork, still artwork, the cover or the artist's header photo, one colour from the cover, or nothing behind the lyrics."
          keywords="background ambience dynamic static colour color"
        >
          <select
            className="settings-select"
            aria-label="Lyrics background"
            value={settings.lyrics_background ?? "animated"}
            onChange={(event) =>
              onSettings({
                lyrics_background: event.target
                  .value as Settings["lyrics_background"],
              })
            }
          >
            <option value="animated">Animated artwork</option>
            <option value="static">Still artwork</option>
            <option value="cover">Cover art</option>
            <option value="artist">Artist header</option>
            <option value="color">Cover colour</option>
            <option value="none">None</option>
          </select>
        </SettingRow>,
        range(
          "lyrics_background_blur",
          "Background blur",
          "Blur the Cover art and Artist header backgrounds.",
          "background blur cover artist header image",
          [0, 48, 1],
          0,
          " px",
          !["cover", "artist"].includes(settings.lyrics_background ?? ""),
        ),
        <SettingRow
          key="pinned-footer"
          label="Pinned Lyrics Footer"
          help="Keep source and community credits visible. Full also pins writers."
          keywords="pinned footer written by writers credits source community"
        >
          <select
            className="settings-select"
            aria-label="Pinned Lyrics Footer"
            value={settings.lyrics_pinned_footer ?? "off"}
            onChange={(event) =>
              onSettings({
                lyrics_pinned_footer: event.target
                  .value as Settings["lyrics_pinned_footer"],
              })
            }
          >
            <option value="off">Off</option>
            <option value="no_writers">No Writers</option>
            <option value="full">Full</option>
          </select>
        </SettingRow>,
        <SettingRow
          key="year"
          label="Release year position"
          help="Show the year before or after the artists beside the lyrics, or leave it out."
          keywords="year date metadata release hide"
        >
          <select
            className="settings-select"
            aria-label="Release year position"
            value={settings.lyrics_year_position ?? "left"}
            onChange={(event) =>
              onSettings({
                lyrics_year_position: event.target
                  .value as Settings["lyrics_year_position"],
              })
            }
          >
            <option value="off">Off</option>
            <option value="left">Left</option>
            <option value="right">Right</option>
          </select>
        </SettingRow>,
      ],
    ],
    [
      "Lyrics",
      "Display",
      [
        flag(
          "lyrics_discord_usernames",
          "Discord usernames in credits",
          "Credit community lyrics makers by their Discord username, looked up on spicylyrics.org, instead of their display name.",
          "discord username display name community credits maker uploader",
        ),
        flag(
          "lyrics_profiles_in_browser",
          "Open profiles in browser",
          "Open community lyrics makers' spicylyrics.org profiles in your browser instead of in Lyssna.",
          "profile browser community credits maker spicylyrics website",
        ),
        range(
          "lyric_offset_ms",
          "Timing offset",
          "Shift lyrics earlier or later to match what you hear.",
          "lyrics sync delay offset milliseconds",
          [-2000, 2000, 10],
          0,
          " ms",
        ),
        flag(
          "lyrics_seek_compensation",
          "Seek fade-in compensation",
          "Clicking a lyric starts 300 ms before it, so a speaker that fades in after a seek is at full volume when the line begins.",
          "seek fade in compensation click lyric early volume",
        ),
        flag(
          "romanized",
          "Show romanized lyrics",
          "Display Latin-alphabet readings. Supplied readings are preferred; automatic transliteration is approximate.",
          "romanization transliteration",
        ),
        flag(
          "lyrics_translations",
          "Show translations",
          "Show translations that come with the lyrics under each line.",
          "translation translate language",
        ),
        flag(
          "experimental_word_sync",
          "Experimental word sync",
          "Estimate word timing for line-synced or plain lyrics. Timing may be off.",
          "word timing estimate",
        ),
        flag(
          "lyrics_simple",
          "Simple lyrics mode",
          "Fill lines without the per-word lift, scale and glow.",
          "simple minimal animation",
        ),
        flag(
          "lyrics_minimal",
          "Minimal lyrics mode",
          "In full screen, sung lines fade away and the lines still to come stay dim.",
          "minimal fullscreen full screen hide sung",
        ),
        flag(
          "lyrics_keep_overlaps",
          "Keep overlapping lines",
          "In Minimal lyrics mode, a line sung over an earlier one stays dimmed instead of fading away.",
          "minimal overlapping lines background vocals keep",
        ),
        flag(
          "lyrics_early_scroll",
          "Early scroll",
          "Start moving to the next line a moment before it is sung.",
          "scroll ahead",
        ),
        range(
          "lyrics_early_scroll_ms",
          "Early scroll time",
          "How far ahead Early scroll moves to the next line.",
          "scroll ahead lead milliseconds",
          [0, 800, 10],
          200,
          " ms",
          !settings.lyrics_early_scroll,
        ),
        flag(
          "lyrics_smooth_scroll",
          "Smooth scrolling",
          "Glide between lines on a spring. Off uses plain smooth scrolling.",
          "smooth scroll spring glide",
        ),
        flag(
          "lyrics_follow_button",
          "Show scroll to active button",
          "While you scroll through lyrics, show a button that returns to the current line.",
          "scroll return follow current line button arrow",
        ),
        flag(
          "lyrics_right_align",
          "Right align lyrics",
          "Line lyrics up on the right instead of the left.",
          "alignment",
        ),
        flag(
          "lyrics_flip_duet",
          "Flip duet sides",
          "Swap which side each duet voice sits on.",
          "duet opposite alignment",
        ),
        flag(
          "lyrics_wide_duet",
          "Wide duet line padding",
          "Separate duet voices into wider columns.",
          "duet padding",
        ),
        flag(
          "lyrics_credits_follow",
          "Credits follow the last singer",
          "Put the credits on the side of the song's last line, so a duet ending on the right keeps them there.",
          "credits duet side align right",
        ),
        flag(
          "lyrics_line_hover",
          "Line hover background",
          "Highlight a line when you point at it.",
          "hover highlight",
        ),
        flag(
          "lyrics_skeleton",
          "Lyrics loading skeleton",
          "Show placeholder lines while lyrics load, instead of a message.",
          "loading placeholder skeleton",
        ),
        flag(
          "lyrics_no_lyrics_notice",
          "No lyrics notice",
          "Say when a song has no lyrics. Off shows only the song; problems reaching a source still show.",
          "no lyrics notice message unavailable missing",
        ),
      ],
    ],
    [
      "Effects",
      "Lyric effects",
      [
        flag(
          "space_gravity",
          "Space Gravity",
          "Words float in zero gravity, drift as they're sung and react when you move the window.",
          "gravity float",
        ),
        flag(
          "lyrics_highlighter",
          "Highlighter",
          "A marker pen sweeps behind each line as it's sung, row by row, and over each floating word in Space Gravity.",
          "highlighter marker highlight pen lyrics player style",
        ),
        settings.lyrics_highlighter ? (
          <MarkerColourRow
            key="highlighter-colour"
            value={settings.lyrics_highlighter_color ?? DEFAULT_MARKER}
            onChange={(lyrics_highlighter_color) =>
              onSettings({ lyrics_highlighter_color })
            }
          />
        ) : null,
        ...(settings.lyrics_highlighter && settings.space_gravity
          ? [
              flag(
                "gravity_links",
                "Connecting lines",
                "In Space Gravity, a marker line joins each sung word to the next.",
                "highlighter space gravity line link connect brainstorm",
              ),
              range(
                "gravity_link_width",
                "Connecting line thickness",
                "How broad the lines are, as a share of the Highlighter's band.",
                "highlighter space gravity line link thickness width",
                [10, 100, 5],
                100,
                "%",
                settings.gravity_links === false,
              ),
            ]
          : []),
        <AnimatorPresetRow
          key="animator"
          animator={settings.lyrics_animator}
          onChange={(lyrics_animator) => onSettings({ lyrics_animator })}
          onManage={onManageAnimator}
        />,
        <SettingRow
          key="word-filter"
          label="Unique word filters"
          help="Show every lyric word in lower case or upper case, or respell lyrics as gibberish the way they sound."
          keywords="word filter lowercase uppercase case meme gibberish wenomechainsama"
        >
          <select
            className="settings-select"
            aria-label="Unique word filters"
            value={settings.lyrics_word_filter ?? "off"}
            onChange={(event) =>
              onSettings({
                lyrics_word_filter: event.target
                  .value as Settings["lyrics_word_filter"],
              })
            }
          >
            <option value="off">Off</option>
            <option value="gibberish">Gibberish</option>
            <option value="lower">all lowercase</option>
            <option value="upper">ALL UPPERCASE</option>
          </select>
        </SettingRow>,
        flag(
          "canvas_lyrics",
          "Canvas in lyrics",
          "Play the track's Canvas video behind the lyrics page.",
          "video background",
        ),
        flag(
          "npv_ambience",
          "Dynamic background in Now playing",
          "Draw the moving artwork, or the Canvas, behind the Now playing panel.",
          "now playing sidebar ambience background dynamic",
        ),
        flag(
          "glass_seek",
          "Glass seek bar",
          "Give the full screen seek bar a frosted glass track and a glowing fill.",
          "seek bar slider progress glass styling fullscreen full screen",
        ),
        flag(
          "cover_animation",
          "Cover art animation",
          "Crossfade the artwork beside the lyrics when the song changes.",
          "cover artwork album fade transition",
        ),
      ],
    ],
    [
      "Layout",
      "Lyrics layout",
      [
        flag(
          "cinema_lyrics",
          "Enable Cinema Lyrics Window",
          "Show or hide the Cinema Lyrics button in the playback bar. It opens the full screen lyrics in a window you can move and resize.",
          "cinema window external popup big",
        ),
        flag(
          "lyrics_compact",
          "Compact lyrics",
          "Use the narrow lyrics layout at any window size.",
          "narrow layout",
        ),
        flag(
          "lyrics_hide_artwork",
          "Hide artwork",
          "Show only the lyrics, without the album cover.",
          "cover",
        ),
        flag(
          "lyrics_artwork_right",
          "Artwork on the right",
          "Put the album cover on the right of the lyrics.",
          "cover side swap",
        ),
        <SettingRow
          key="toolbar-position"
          label="View controls position"
          help="Where the lyrics toolbar sits outside full screen."
          keywords="toolbar view controls top bottom position"
        >
          <select
            className="settings-select"
            aria-label="View controls position"
            value={settings.lyrics_toolbar_bottom ? "bottom" : "top"}
            onChange={(event) =>
              onSettings({
                lyrics_toolbar_bottom: event.target.value === "bottom",
              })
            }
          >
            <option value="top">Top</option>
            <option value="bottom">Bottom</option>
          </select>
        </SettingRow>,
        <SettingRow
          key="always-show"
          label="Always show in full screen"
          help="Keep the playback controls, the time, or both on screen instead of hiding them when the mouse rests."
          keywords="fullscreen full screen controls time seek hide idle"
        >
          <select
            className="settings-select"
            aria-label="Always show in full screen"
            value={settings.fullscreen_always_show ?? "none"}
            onChange={(event) =>
              onSettings({
                fullscreen_always_show: event.target
                  .value as Settings["fullscreen_always_show"],
              })
            }
          >
            <option value="none">None</option>
            <option value="controls">Controls</option>
            <option value="time">Time</option>
            <option value="both">Both</option>
          </select>
        </SettingRow>,
        <SettingRow
          key="fullscreen-volume"
          label="Full screen volume slider"
          help="Add a volume slider to the full screen controls."
          keywords="fullscreen full screen volume slider sound"
        >
          <select
            className="settings-select"
            aria-label="Full screen volume slider"
            value={settings.fullscreen_volume ?? "off"}
            onChange={(event) =>
              onSettings({
                fullscreen_volume: event.target
                  .value as Settings["fullscreen_volume"],
              })
            }
          >
            <option value="off">Off</option>
            <option value="left">Left</option>
            <option value="right">Right</option>
            <option value="below">Below</option>
          </select>
        </SettingRow>,
        <SettingRow
          key="escape"
          label="Escape key function"
          help="What Escape does on the lyrics page: leave full screen only, or leave the lyrics too."
          keywords="escape key esc exit fullscreen full screen lyrics close"
        >
          <select
            className="settings-select"
            aria-label="Escape key function"
            value={settings.lyrics_escape_exits ? "fully" : "fullscreen"}
            onChange={(event) =>
              onSettings({
                lyrics_escape_exits: event.target.value === "fully",
              })
            }
          >
            <option value="fullscreen">Exit full screen</option>
            <option value="fully">Exit fully</option>
          </select>
        </SettingRow>,
        flag(
          "fullscreen_animate_close",
          "Animate closing full screen",
          "Slide the lyrics away before full screen closes.",
          "fullscreen full screen close exit animation slide",
        ),
        flag(
          "lyrics_hide_npv",
          "Hide lyrics in Now playing",
          "Never show the lyrics card in the Now playing panel.",
          "now playing card sidebar",
        ),
        flag(
          "lyrics_hide_npv_empty",
          "Hide the Now playing card without lyrics",
          "Remove the lyrics card when the current song has no lyrics.",
          "now playing card empty",
        ),
      ],
    ],
    [
      "Sources",
      "Lyric sources",
      [
        <SettingRow
          key="sources"
          stacked
          label="Lyric sources"
          help="Tried top to bottom until one has lyrics for the song."
          keywords="sources priority order lrclib spicy provider"
        >
          <ol className="lyric-sources">
            {order.map((id, index) => {
              const source = SOURCES[id];
              if (!source) return null;
              const on = !disabled.has(id);
              return (
                <li key={id} className={on ? "" : "off"}>
                  <span className="lyric-source-name">
                    <strong>{source.name}</strong>
                    <small>{source.help}</small>
                  </span>
                  <button
                    className="icon-button"
                    title={`Move ${source.name} up`}
                    disabled={index === 0}
                    onClick={() => move(id, -1)}
                  >
                    <ChevronUp size={16} />
                  </button>
                  <button
                    className="icon-button"
                    title={`Move ${source.name} down`}
                    disabled={index === order.length - 1}
                    onClick={() => move(id, 1)}
                  >
                    <ChevronDown size={16} />
                  </button>
                  <Toggle
                    label={source.name}
                    checked={on}
                    onChange={(checked) => {
                      const next = new Set(disabled);
                      if (checked) next.delete(id);
                      else next.add(id);
                      onSettings({ lyrics_sources_disabled: [...next] });
                    }}
                  />
                </li>
              );
            })}
          </ol>
          <button
            className="text-link lyric-sources-reset"
            onClick={() =>
              onSettings({
                lyrics_source_order: DEFAULT_SOURCES,
                lyrics_sources_disabled: DEFAULT_SOURCES_OFF,
              })
            }
          >
            Reset sources
          </button>
        </SettingRow>,
      ],
    ],
    [
      "Sources",
      "Source options",
      [
        flag(
          "lyrics_prefer_apple",
          "Prioritize Apple Music quality",
          "Use Apple Music's lyrics when they are better timed than the chosen source, or as well timed with real pauses between lines.",
          "apple music quality priority source",
        ),
        flag(
          "lyrics_musixmatch_lines_only",
          "Ignore Musixmatch word sync",
          "Use only Musixmatch's line timing, even when it has word timing.",
          "musixmatch word sync richsync line timing",
        ),
        <MusixmatchToken
          key="musixmatch"
          onCall={onCall}
          onNotice={onNotice}
        />,
      ],
    ],
    [
      "Sources",
      "Your lyrics",
      [
        <LyricsProvider
          key="provider"
          state={state}
          onCall={onCall}
          onNotice={onNotice}
        />,
        <SettingRow
          key="ttml"
          label="Load TTML"
          help="Use your own lyrics file for the playing song."
          keywords="import file"
        >
          <button className="pill outline" onClick={onLoadTtml}>
            Load
          </button>
        </SettingRow>,
        flag(
          "lyrics_cache_button",
          "Lyrics cache button",
          "Show the refresh button in the lyrics toolbar.",
          "cache refresh button toolbar clear reload",
        ),
        <SettingRow
          key="cache-action"
          label="Lyrics cache button action"
          help="What the toolbar's refresh button clears before fetching the song's lyrics again."
          keywords="cache refresh button toolbar clear reload saved"
        >
          <select
            className="settings-select"
            aria-label="Lyrics cache button action"
            value={settings.lyrics_cache_action ?? "song"}
            onChange={(event) =>
              onSettings({
                lyrics_cache_action: event.target
                  .value as Settings["lyrics_cache_action"],
              })
            }
          >
            <option value="song">Clear current song</option>
            <option value="reload">Reload current song</option>
            <option value="all">Clear all saved lyrics</option>
          </select>
        </SettingRow>,
        <SettingRow
          key="database"
          label="Browse TTML Database"
          help="Lyrics you loaded, with play, download and delete."
          keywords="saved lyrics database"
        >
          <button className="pill outline" onClick={onDatabase}>
            Browse
          </button>
        </SettingRow>,
      ],
    ],
    [
      "About",
      "About",
      [
        <SettingRow
          key="credits"
          stacked
          label="Credits and licences"
          keywords={`about credits licence license ${CREDITS.map((credit) => `${credit.name} ${credit.by}`).join(" ")}`}
        >
          <Credits onOpen={onOpen} />
        </SettingRow>,
      ],
    ],
    [
      "Advanced",
      "Builds",
      [
        <SettingRow
          key="update-channel"
          label="Version"
          help={
            nightlyChosen
              ? "Nightly: the newest changes, built by Lyssna on this computer from the latest code."
              : "Stable: tested releases that update automatically."
          }
          keywords="build version channel stable nightly beta updates source"
        >
          <select
            className="settings-select"
            aria-label="Version"
            value={nightlyChosen ? "nightly" : "stable"}
            onChange={(event) => {
              if (event.target.value === "nightly") setNightlyOpen(true);
              else {
                if (nightlyBuilding) {
                  setNightlyStarting(false);
                  void onCall("nightly_cancel");
                }
                onSettings({ update_channel: "stable" });
              }
            }}
          >
            <option value="stable">Stable</option>
            <option value="nightly">Nightly</option>
          </select>
        </SettingRow>,
        nightly || nightlyBuilding ? (
          <SettingRow
            key="nightly-build"
            label={
              nightlyBuilding
                ? `Building Nightly · ${nightlyReport ? Math.round(nightlyProgress(nightlyReport) * 100) : 0}%`
                : "Nightly build"
            }
            help={
              nightlyBuilding
                ? "Lyssna restarts into it when it's done."
                : "Build the newest code now."
            }
            keywords="nightly build latest source update"
          >
            <button
              className="pill outline"
              onClick={() =>
                nightlyBuilding ? setNightlyOpen(true) : buildNightly()
              }
            >
              {nightlyBuilding ? "Show progress" : "Build latest"}
            </button>
          </SettingRow>
        ) : null,
        nightly ? (
          <SettingRow
            key="nightly-cache"
            label="Nightly build cache"
            help="Files kept so the next build is quicker. Clear them if a build keeps failing; the next build starts from scratch and takes longer. Rust and the other build tools are kept."
            keywords="nightly build cache clear target source disk space failing"
          >
            <button
              className="pill outline"
              disabled={clearingNightly || nightlyBuilding}
              onClick={() => {
                setClearingNightly(true);
                void onCall<number>("nightly_clear_cache")
                  .then((freed) => {
                    if (freed !== undefined)
                      onNotice(
                        `Cleared ${Math.round(freed / 1_048_576).toLocaleString()} MB of Nightly build files.`,
                      );
                  })
                  .finally(() => setClearingNightly(false));
              }}
            >
              Clear cache
            </button>
          </SettingRow>
        ) : null,
      ],
    ],
    [
      "Advanced",
      "Settings window",
      [
        flag(
          "settings_remember_place",
          "Remember settings location",
          "Reopen Settings on the tab and at the place it was closed.",
          "settings menu tab location remember reopen",
        ),
        flag(
          "settings_allow_hiding",
          "Allow hiding settings",
          "Hide settings you never change. Hidden ones are listed below, where they can be shown again.",
          "hide hidden settings manage",
        ),
        settings.settings_allow_hiding ? (
          <SettingRow
            key="hide-buttons"
            label="Show hide buttons"
            help="Put a hide button beside each setting's name."
            keywords="hide hidden settings buttons eye"
          >
            <Toggle
              label="Show hide buttons"
              checked={settings.settings_hide_buttons !== false}
              onChange={(checked) =>
                onSettings({ settings_hide_buttons: checked })
              }
            />
          </SettingRow>
        ) : null,
      ],
    ],
    [
      "Advanced",
      "Developer",
      [
        flag(
          "developer_mode",
          "Developer mode",
          "Allow the developer tools, opened with F12 or the button below, for inspecting the interface.",
          "developer mode devtools debug inspect console",
        ),
        settings.developer_mode ? (
          <SettingRow
            key="devtools"
            label="Developer tools"
            help="Inspect Lyssna's interface. Anything you run there acts as you."
            keywords="developer devtools debug inspect console"
          >
            <button
              className="pill outline"
              onClick={() => void onCall("open_devtools")}
            >
              Open
            </button>
          </SettingRow>
        ) : null,
      ],
    ],
  ];
  // Rows built from SettingRow can be hidden, named by their React key.
  const hidden = new Set(settings.hidden_settings ?? []);
  const named: { id: string; label: string }[] = [];
  // Advanced rows stay, so hiding can always be turned off again.
  for (const section of sections.filter(([tab]) => tab !== "Advanced"))
    section[2] = section[2].map((row) => {
      if (!isValidElement(row) || row.type !== SettingRow || row.key === null)
        return row;
      const id = String(row.key),
        label = (row as ReactElement<{ label: unknown }>).props.label;
      named.push({ id, label: typeof label === "string" ? label : id });
      return cloneElement(row as ReactElement<{ hideId?: string }>, {
        hideId: id,
      });
    });
  const hiding = {
    allow: !!settings.settings_allow_hiding,
    buttons: settings.settings_hide_buttons !== false,
    hidden,
    hide: (id: string) =>
      onSettings({ hidden_settings: [...new Set([...hidden, id])] }),
  };
  const hiddenRows = named.filter(({ id }) => hidden.has(id));
  if (hiding.allow)
    sections.push([
      "Advanced",
      "Hidden settings",
      hiddenRows.length
        ? hiddenRows.map(({ id, label }) => (
            <SettingRow key={`hidden-${id}`} label={label} keywords="hidden">
              <button
                className="pill outline"
                onClick={() =>
                  onSettings({
                    hidden_settings: [...hidden].filter((item) => item !== id),
                  })
                }
              >
                Show
              </button>
            </SettingRow>
          ))
        : [
            <SettingRow
              key="hidden-none"
              label="No hidden settings"
              help="Use the hide button beside a setting's name to hide it."
              keywords="hidden"
            />,
          ],
    ]);
  const about = tab === "About" && !searching;
  const visible = sections.filter(([key]) => searching || key === tab);
  const status =
    state.update_status === "Up to date"
      ? ["latest", "Latest"]
      : state.update_status === "Update available" ||
          state.update_status === "Ready to install"
        ? ["outdated", "Update available"]
        : undefined;
  return (
    <div className="modal-backdrop lyrics-settings-backdrop" onClick={onClose}>
      <section
        className="lyrics-settings"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-window-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="lyrics-settings-head">
          <h2 id="settings-window-title">
            <Settings2 size={20} />
            Lyssna settings
          </h2>
          <label className="lyrics-settings-search">
            <Search size={15} />
            <input
              aria-label="Search settings"
              placeholder="Search"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                scroll.current?.scrollTo({ top: 0 });
              }}
            />
          </label>
          <button
            className="icon-button"
            aria-label="Close settings"
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </header>
        <div
          className="lyrics-settings-tabs"
          role="tablist"
          aria-label="Settings categories"
        >
          <span
            className="lyrics-settings-marker"
            aria-hidden="true"
            style={
              marker && !searching && tab !== "About"
                ? {
                    opacity: 1,
                    transform: `translateX(${marker.left}px)`,
                    width: marker.width,
                  }
                : { opacity: 0 }
            }
          />
          {TABS.map(([label, Icon], index) => (
            <button
              key={label}
              ref={(element) => {
                tabs.current[index] = element;
              }}
              role="tab"
              aria-selected={tab === label && !searching}
              className={tab === label && !searching ? "active" : ""}
              onClick={() => select(label)}
              onKeyDown={(event) => {
                const step =
                  event.key === "ArrowRight"
                    ? 1
                    : event.key === "ArrowLeft"
                      ? -1
                      : 0;
                if (!step) return;
                event.preventDefault();
                const next = (index + step + TABS.length) % TABS.length;
                select(TABS[next][0]);
                tabs.current[next]?.focus();
              }}
            >
              <Icon size={16} />
              {label}
            </button>
          ))}
        </div>
        <div
          className="lyrics-settings-scroll"
          ref={scroll}
          onScroll={(event) => {
            if (!searching) writePlace(tab, event.currentTarget.scrollTop);
          }}
        >
          <HiddenSettings.Provider value={hiding}>
            <SettingsQuery.Provider value={query}>
              {about ? (
                <section className="lyrics-settings-about" aria-label="About">
                  <button
                    className="text-link lyrics-settings-back"
                    onClick={() => select("General")}
                  >
                    <ChevronLeft size={16} />
                    Settings
                  </button>
                  <Credits onOpen={onOpen} />
                </section>
              ) : (
                <div role="tabpanel">
                  {visible.map(([key, title, rows]) => (
                    <section
                      key={`${key}-${title}`}
                      className="lyrics-settings-section"
                    >
                      <h3>{title}</h3>
                      <div className="lyrics-settings-group">{rows}</div>
                    </section>
                  ))}
                  <p className="lyrics-settings-empty">No settings match.</p>
                </div>
              )}
            </SettingsQuery.Provider>
          </HiddenSettings.Provider>
          <footer className="lyrics-settings-footer">
            <button
              className="lyrics-settings-about-link"
              aria-pressed={about}
              onClick={() => select(about ? "General" : "About")}
            >
              <span className="lyrics-settings-wordmark">Lyssna</span>
              <span className="lyrics-settings-version">
                v{__APP_VERSION__}
                {status && (
                  <span className={`lyrics-settings-status ${status[0]}`}>
                    {status[1]}
                  </span>
                )}
              </span>
              <span className="lyrics-settings-about-hint">
                {about ? "Back to settings" : "About and credits"}
                <ChevronRight size={14} />
              </span>
            </button>
            <span className="lyrics-settings-build">
              {import.meta.env.DEV
                ? "Development build"
                : nightlyStatus?.commit
                  ? `Nightly build · ${nightlyStatus.commit.slice(0, 7)}`
                  : "Release build"}
            </span>
          </footer>
        </div>
      </section>
      {nightlyOpen && (
        <NightlyDialog
          report={nightlyStarting ? null : nightlyReport}
          starting={nightlyStarting}
          onStart={buildNightly}
          onCancelBuild={() => void onCall("nightly_cancel")}
          onHide={() => setNightlyOpen(false)}
          onLog={() => void onCall("nightly_log")}
        />
      )}
    </div>
  );
}

/** The Highlighter's marker colour: a preset, the cover's colour, or any
 *  colour from the picker. */
function MarkerColourRow({
  value,
  onChange,
}: {
  value: string;
  onChange: (colour: string) => void;
}) {
  const preset =
    value === "cover" || MARKER_COLOURS.some(([hex]) => hex === value)
      ? value
      : "custom";
  return (
    <SettingRow
      label="Marker colour"
      help="The Highlighter's ink. Cover colour follows the artwork; Custom takes any colour."
      keywords="highlighter marker colour color ink custom cover"
    >
      <span className="marker-colour">
        <select
          className="settings-select"
          aria-label="Marker colour"
          value={preset}
          onChange={(event) =>
            onChange(
              event.target.value === "custom"
                ? value === "cover"
                  ? DEFAULT_MARKER
                  : value
                : event.target.value,
            )
          }
        >
          {MARKER_COLOURS.map(([hex, name]) => (
            <option key={hex} value={hex}>
              {name}
            </option>
          ))}
          <option value="cover">Cover colour</option>
          <option value="custom">Custom</option>
        </select>
        <input
          type="color"
          aria-label="Custom marker colour"
          value={value === "cover" ? DEFAULT_MARKER : value}
          disabled={value === "cover"}
          onChange={(event) => onChange(event.target.value)}
        />
      </span>
    </SettingRow>
  );
}
