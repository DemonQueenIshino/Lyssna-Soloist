import { contextName } from "./playing-context";
import {
  memo,
  Fragment,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { artworkFor, isLocalCover } from "./artwork";
import { LyricsCanvas } from "./canvas-lyrics";
import { SpaceGravityLyrics } from "./space-gravity-lyrics";
import { displayLyrics } from "./word-sync";
import { useRomanization } from "./romanization";
import { filterWords } from "./word-filter";
import { frameDue } from "./frame-cap";
import { useArtworkColor } from "./artwork-color";
import {
  Music2,
  Maximize2,
  Upload,
  RotateCw,
  Mic2,
  CirclePlus,
  CircleCheck,
  ChevronUp,
  ChevronDown,
  X,
  PanelLeftOpen,
  UnfoldVertical,
  FoldVertical,
  Copy,
} from "lucide-react";
import { Toggle } from "./setting-row";
import { VolumeControl } from "./volume-control";
import {
  FullscreenHeartIcon,
  FullscreenPauseIcon,
  FullscreenPlayIcon,
  FullscreenRepeatIcon,
  FullscreenShuffleIcon,
  FullscreenVolumeIcon,
  SkipNextIcon,
  SkipPreviousIcon,
} from "./fullscreen-icons";
import { DJArtwork, DJ_COVER } from "./brand";
import type {
  Album,
  Artist,
  Contributor,
  Track,
  PlayerRequest,
  Lyrics,
  Settings,
  Line,
  Jam,
} from "./types";
import type { AppState } from "./state";
import { activeLines, endsOnRight, formatTime } from "./state";
import { PlaybackClock } from "./playback-clock";
import { rpc } from "./bridge";
import { CreditsDialog } from "./credits-dialog";
import { useCreatorProfile } from "./creator-profile";
import {
  stepScroll,
  vocalGaps,
  type VocalGap,
  wordGroups,
  vocalGap,
  vocalEnd,
  lineBlur,
  tuneMotion,
  glideScroll,
  GLIDE_MS,
  type ScrollSpring,
} from "./lyric-motion";
import { LyricAnimator } from "./lyric-animator";
import { HighlighterWords, markerColour } from "./highlighter";
import { LyricWord } from "./lyric-word";
import { LyricCountdown } from "./lyric-countdown";

/** Artwork is HTTPS, a local JPEG preview of a cover being saved, or a
 * Local Files cover. */
export const usableImage = (url: string) =>
  url.startsWith("https://") ||
  url.startsWith("data:image/jpeg;base64,") ||
  isLocalCover(url);
export const imageOf = (images?: { url: string }[]) =>
  images?.find((image) => usableImage(image.url))?.url;
export const artistNames = (track?: Track) =>
  track?.artists.map((artist) => artist.name).join(", ") ?? "";
// Cover sizes are read during layout effects and applied together after
// them, so setting one image's src never forces layout for the next read.
const pendingSources: [HTMLImageElement, string][] = [];
function setSourceSoon(element: HTMLImageElement, url: string) {
  if (!pendingSources.length)
    queueMicrotask(() => {
      for (const [image, source] of pendingSources.splice(0))
        if (image.getAttribute("src") !== source) image.src = source;
    });
  pendingSources.push([element, url]);
}
/** Hero title size tier: longer names step down, like Spotify's headers. */
export const titleSize = (title: string) =>
  title.length > 28 ? "long-title" : title.length > 14 ? "medium-title" : "";

/** Whether track artwork crossfades on change: Cover Art Animation. */
export const coverFade = (settings: Settings) =>
  settings.cover_animation !== false && !settings.reduce_motion;

export function Cover({
  images,
  className = "",
  onClick,
  contextRoute,
  contextName,
  fade = false,
}: {
  images?: { url: string; width?: number | null }[];
  className?: string;
  onClick?: () => void;
  contextRoute?: string;
  contextName?: string;
  /** Crossfade from the old artwork when it changes (Cover Art Animation). */
  fade?: boolean;
}) {
  const src = imageOf(images);
  const picture = useRef<HTMLImageElement>(null);
  const attempted = useRef(new Set<string>());
  const [failed, setFailed] = useState(false);
  // The artwork being replaced, held over the new one until it has loaded.
  const [previous, setPrevious] = useState<{ url: string; fading: boolean }>();
  const sources = images?.map((image) => image.url).join(" ");
  // Load the smallest Spotify size that fills the laid-out cover. Set here,
  // not through srcset/sizes="auto": Chromium keeps every image with
  // sizes="auto" (and its page) alive after it leaves the page.
  useLayoutEffect(() => {
    attempted.current.clear();
    setFailed(false);
    const element = picture.current;
    if (!element) return;
    if (fade && element.complete && element.currentSrc)
      setPrevious({ url: element.currentSrc, fading: false });
    const width = element.parentElement?.clientWidth ?? 0;
    const url = artworkFor(
      images,
      width ? Math.ceil(width * devicePixelRatio) : undefined,
    );
    if (url) {
      attempted.current.add(url);
      setSourceSoon(element, url);
    }
  }, [sources]);
  const inner = src ? (
    <>
      <img
        ref={picture}
        alt=""
        draggable={false}
        loading="lazy"
        decoding="async"
        style={failed ? { display: "none" } : undefined}
        onLoad={() =>
          setPrevious((old) => old && { url: old.url, fading: true })
        }
        onError={(event) => {
          const next = images?.find(
            (image) =>
              usableImage(image.url) && !attempted.current.has(image.url),
          )?.url;
          if (next) {
            attempted.current.add(next);
            event.currentTarget.src = next;
          } else {
            setFailed(true);
            setPrevious(undefined);
          }
        }}
      />
      {previous && (
        <img
          className={`cover-previous ${previous.fading ? "fading" : ""}`}
          src={previous.url}
          alt=""
          draggable={false}
          onAnimationEnd={() => setPrevious(undefined)}
        />
      )}
      {failed && <Music2 aria-hidden="true" />}
    </>
  ) : (
    <Music2 aria-hidden="true" />
  );
  return onClick ? (
    <button
      className={`cover ${className}`}
      onClick={onClick}
      aria-label="Open album"
      data-context-route={contextRoute}
      data-context-name={contextName}
    >
      {inner}
    </button>
  ) : (
    <div
      className={`cover ${className}`}
      data-context-route={contextRoute}
      data-context-name={contextName}
    >
      {inner}
    </div>
  );
}
import { Ambience } from "./ambience";
export { Ambience };

export function Marquee({
  children,
  text,
  className = "",
  reduceMotion = false,
}: {
  children: ReactNode;
  text: string;
  className?: string;
  reduceMotion?: boolean;
}) {
  const box = useRef<HTMLDivElement>(null),
    content = useRef<HTMLDivElement>(null);
  const [overflow, setOverflow] = useState(0);
  useLayoutEffect(() => {
    const update = () =>
      setOverflow(
        Math.max(
          0,
          (content.current?.scrollWidth ?? 0) - (box.current?.clientWidth ?? 0),
        ),
      );
    update();
    const observer = new ResizeObserver(update);
    if (box.current) observer.observe(box.current);
    if (content.current) observer.observe(content.current);
    return () => observer.disconnect();
  }, [text]);
  return (
    <div
      className={`marquee ${overflow > 2 && !reduceMotion ? "scrolling" : ""} ${className}`}
      ref={box}
      title={text}
      style={
        {
          "--travel": `${overflow}px`,
          "--duration": `${Math.max(20, (overflow * 2) / 14 + 10)}s`,
        } as CSSProperties
      }
    >
      <div key={text} className="marquee-content" ref={content}>
        {children}
      </div>
    </div>
  );
}
// Load a page while the pointer rests on its link so opening it is instant.
// One background load at a time so it never delays a real click.
let hoverTimer: ReturnType<typeof setTimeout> | undefined;
let lastPrefetch = 0;
const prefetched = new Set<string>();
export function prefetchPage(key: string) {
  clearTimeout(hoverTimer);
  if (prefetched.has(key)) return;
  hoverTimer = setTimeout(() => {
    if (Date.now() - lastPrefetch < 1200) return;
    lastPrefetch = Date.now();
    prefetched.add(key);
    if (prefetched.size > 200) prefetched.clear();
    void rpc("prefetch_page", { key }).catch(() => prefetched.delete(key));
  }, 180);
}
export const cancelPrefetch = () => clearTimeout(hoverTimer);

const decodedImages = new Set<string>();
const imageLoads = new Map<string, Promise<boolean>>();
export function preloadImage(url?: string): Promise<boolean> {
  if (!url) return Promise.resolve(false);
  if (decodedImages.has(url)) return Promise.resolve(true);
  const existing = imageLoads.get(url);
  if (existing) return existing;
  const request = new Promise<boolean>((resolve) => {
    const image = new Image();
    const finish = (success: boolean) => {
      image.onload = null;
      image.onerror = null;
      imageLoads.delete(url);
      if (success) {
        decodedImages.add(url);
        if (decodedImages.size > 512)
          decodedImages.delete(decodedImages.values().next().value!);
      }
      resolve(success);
    };
    image.onload = () => {
      void image
        .decode()
        .catch(() => {})
        .then(() => finish(true));
    };
    image.onerror = () => finish(false);
    image.src = url;
  });
  imageLoads.set(url, request);
  return request;
}
export function useLoadedImage(url?: string) {
  const [loaded, setLoaded] = useState<string>();
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    void preloadImage(url).then((success) => {
      if (success && !cancelled) setLoaded(url);
    });
    return () => {
      cancelled = true;
    };
  }, [url]);
  return loaded === url || (url && decodedImages.has(url)) ? url : undefined;
}

/**
 * Like useLoadedImage, but while a replacement for the same `key` decodes
 * (or the URL briefly disappears) it keeps returning the last decoded image,
 * so refreshed metadata does not flash an empty background.
 */
export function useStickyLoadedImage(url: string | undefined, key: string) {
  const loaded = useLoadedImage(url);
  const [last, setLast] = useState<{ key: string; url: string }>();
  useEffect(() => {
    if (loaded) setLast({ key, url: loaded });
  }, [loaded, key]);
  return loaded ?? (last?.key === key ? last.url : undefined);
}

export const ArtistLinks = memo(function ArtistLinks({
  artists,
  onArtist,
}: {
  artists: Artist[];
  onArtist: (artist: Artist) => void;
}) {
  return (
    <>
      {artists.map((artist, index) => (
        <span key={artist.id ?? `${artist.name}-${index}`}>
          {index > 0 && <span className="artist-separator">, </span>}
          <button
            className="text-link artist-link"
            data-context-route={artist.id ? `artist:${artist.id}` : undefined}
            data-context-name={artist.name}
            onClick={() => onArtist(artist)}
            onMouseEnter={() =>
              artist.id && prefetchPage(`artist:${artist.id}`)
            }
            onMouseLeave={cancelPrefetch}
          >
            {artist.name}
          </button>
        </span>
      ))}
    </>
  );
});
/** Songwriter credit as Spicy Lyrics shows it: a dim label, then the
 * names a little brighter. Without names, Spicy Lyrics omits the line. */
function WriterCredits({ names }: { names: string[] }) {
  if (!names.length) return null;
  return (
    <p className="writer-credits">
      <span className="credit-label">Written by</span>{" "}
      <span className="credit-names">{names.join(", ")}</span>
    </p>
  );
}
/** The badge's E is drawn, not typed: a font's ascent and pixel rounding
 * left a typed E off centre at small sizes. */
export const EXPLICIT_MARK = (
  <svg viewBox="0 0 16 16" aria-hidden="true">
    <path d="M5.5 4.2h5.1v1.7H7.3v1.25h3v1.7h-3v1.25h3.3v1.7H5.5z" />
  </svg>
);
/** Spotify's mint verified badge, its tick cut out so the background shows
 * through. */
export function VerifiedBadge({ label }: { label?: string }) {
  const mask = useId();
  return (
    <svg
      className="verified-badge"
      viewBox="0 0 24 24"
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <mask id={mask}>
        <rect width="24" height="24" fill="#fff" />
        <path
          d="m8.5 12 2.5 2.5 4.5-5"
          fill="none"
          stroke="#000"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </mask>
      <path
        mask={`url(#${CSS.escape(mask)})`}
        d="M3.85 8.62a4 4 0 0 1 4.78-4.77 4 4 0 0 1 6.74 0 4 4 0 0 1 4.78 4.78 4 4 0 0 1 0 6.74 4 4 0 0 1-4.77 4.78 4 4 0 0 1-6.75 0 4 4 0 0 1-4.78-4.77 4 4 0 0 1 0-6.76Z"
      />
    </svg>
  );
}

export function ExplicitBadge({ track }: { track: Track }) {
  return track.explicit ? (
    <span className="explicit-badge" aria-label="Explicit" title="Explicit">
      {EXPLICIT_MARK}
    </span>
  ) : null;
}
export const TrackTitle = memo(function TrackTitle({
  track,
  onAlbum,
  reduceMotion = false,
  className = "",
}: {
  track: Track;
  onAlbum: (album: Album) => void;
  reduceMotion?: boolean;
  className?: string;
}) {
  return (
    <Marquee
      text={track.name}
      className={className}
      reduceMotion={reduceMotion}
    >
      <button
        className="text-link song-title"
        data-context-route={track.id ? `track:${track.id}` : undefined}
        onClick={() => onAlbum(track.album)}
      >
        {track.name}
      </button>
    </Marquee>
  );
});

export function usePosition(
  state: Pick<AppState, "playback" | "observed_at" | "clock_revision"> &
    Partial<Pick<AppState, "local_device_name">>,
) {
  const clock = useRef(new PlaybackClock());
  const [time, setTime] = useState(performance.now());
  const now = performance.now();
  clock.current.observe(
    state.playback,
    state.observed_at,
    state.clock_revision,
    now,
    Date.now(),
    state.local_device_name
      ? state.playback.device.name === state.local_device_name
      : true,
  );
  useEffect(() => {
    setTime(performance.now());
    if (!state.playback.is_playing) return;
    let frame = 0,
      shown = 0;
    const tick = (time: number) => {
      if (frameDue(shown, time)) {
        shown = time;
        setTime(time);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [state.playback.is_playing]);
  return clock.current.read(Math.max(time, now));
}

/** Seek Fade-in Compensation from Spicy Lyrics: a clicked lyric starts
 *  this far early, so a device's fade-in after a seek ends as it begins. */
const SEEK_FADE_LEAD_MS = 300;
const lyricSeekTarget = (ms: number) => Math.max(0, ms - SEEK_FADE_LEAD_MS);
/** Spicy Lyrics resumes following this long after the last scroll. */
const BROWSE_COOLDOWN_MS = 750;
export function LyricsDisplay({
  lyrics,
  positionMs,
  settings,
  onSeek,
  compact = false,
  writers = [],
}: {
  lyrics: Lyrics;
  positionMs: number;
  settings: Settings;
  onSeek: (ms: number) => void;
  compact?: boolean;
  writers?: string[];
}) {
  const scroll = useRef<HTMLDivElement>(null);
  const [following, setFollowing] = useState(true);
  const [atEnd, setAtEnd] = useState(false);
  const [returnDirection, setReturnDirection] = useState<"up" | "down">("down");
  const footerMode = compact ? "off" : (settings.lyrics_pinned_footer ?? "off");
  const writerCredits = (
    <WriterCredits
      names={lyrics.songwriters?.length ? lyrics.songwriters : writers}
    />
  );
  // As in Spicy Lyrics, browsing ends on its own once the user stops
  // scrolling with the current lyric still on screen; scrolled away from
  // it, browsing stays until they return or select a lyric.
  const glide = useRef<{ from: number; start: number }>(undefined);
  const browsedAt = useRef(0);
  const browse = () => {
    glide.current = undefined;
    browsedAt.current = performance.now();
    setFollowing(false);
  };
  const lines = useRef<(HTMLButtonElement | null)[]>([]);
  const seek = useRef(onSeek);
  const seekLead = settings.lyrics_seek_compensation !== false;
  useLayoutEffect(() => {
    seek.current = (ms) => onSeek(seekLead ? lyricSeekTarget(ms) : ms);
  }, [onSeek, seekLead]);
  // Clock frames must not replace each button's native click listener or ref.
  const lineActions = useMemo(
    () =>
      lyrics.lines.map((line, index) => ({
        ref: (element: HTMLButtonElement | null) => {
          lines.current[index] = element;
        },
        click: () => {
          if (lyrics.synced) {
            seek.current(line.words[0]?.start_ms ?? line.start_ms);
            setFollowing(true);
          }
        },
      })),
    [lyrics],
  );
  const spring = useRef<ScrollSpring>({ position: 0, velocity: 0, target: 0 });
  const waiting = useRef<HTMLDivElement>(null);
  const gap = lyrics.synced ? vocalGap(lyrics.lines, positionMs) : undefined;
  const gaps = useMemo(
    () => new Map(vocalGaps(lyrics.lines).map((item) => [item.after, item])),
    [lyrics],
  );
  const waitingDots = (item: VocalGap) => {
    const visible = gap?.start === item.start;
    return (
      <LyricCountdown
        key={item.start}
        ref={visible ? waiting : undefined}
        gap={item}
        // Song time only matters near the pause; held still elsewhere, a
        // countdown far off does not render again every frame.
        time={Math.min(
          item.end + COUNTDOWN_REACH_MS,
          Math.max(item.start - COUNTDOWN_REACH_MS, positionMs),
        )}
        reduceMotion={settings.reduce_motion}
        visible={visible}
      />
    );
  };
  // Early scroll follows the line about to be sung; highlighting still
  // follows the real song time.
  const scrollTime =
    positionMs +
    (settings.lyrics_early_scroll
      ? (settings.lyrics_early_scroll_ms ?? 200)
      : 0);
  const active = activeLines(lyrics.lines, scrollTime);
  const primary =
    active.find((index) => !lyrics.lines[index].background) ??
    lyrics.lines.reduce(
      (last, line, index) =>
        line.start_ms <= scrollTime && !line.background ? index : last,
      0,
    );
  useEffect(() => {
    setFollowing(true);
  }, [lyrics]);
  // A restart, Previous or seek moves song time far in one step. Lyrics then
  // slide straight there instead of springing through the whole song.
  const lastPosition = useRef(positionMs),
    jumped = useRef(false);
  const positionJumped = Math.abs(positionMs - lastPosition.current) > 1500;
  if (positionJumped) jumped.current = true;
  lastPosition.current = positionMs;
  const animator = useRef<LyricAnimator>(undefined);
  // New springs pick up the animator preset's curves and tuning.
  const tuning = JSON.stringify(settings.lyrics_animator ?? {});
  useLayoutEffect(() => {
    tuneMotion(JSON.parse(tuning));
    const instance = new LyricAnimator(
      lyrics,
      (index) => lines.current[index] ?? null,
      settings.reduce_motion || !!settings.lyrics_simple,
    );
    animator.current = instance;
    return () => instance.dispose();
  }, [lyrics, settings.reduce_motion, settings.lyrics_simple, tuning]);
  useLayoutEffect(() => {
    if (!lyrics.synced) return;
    if (positionJumped) animator.current?.reset();
    animator.current?.update(positionMs);
  }, [positionMs, lyrics, settings.reduce_motion]);
  // Romanization, translations or a word filter change only the words: the
  // view stays on the current line instead of sliding to where it rewraps.
  const timing = useMemo(
    () => lyrics.lines.map((line) => line.start_ms).join(","),
    [lyrics],
  );
  // The first placement, as the page opens or an effect switches, lands on
  // the current line at once rather than sliding up from the top.
  const placed = useRef(false);
  const wording = useRef({
    lyrics,
    timing,
    translations: settings.lyrics_translations,
  });
  useLayoutEffect(() => {
    const before = wording.current;
    let reworded =
      before.timing === timing &&
      (before.lyrics !== lyrics ||
        before.translations !== settings.lyrics_translations);
    wording.current = {
      lyrics,
      timing,
      translations: settings.lyrics_translations,
    };
    const container = scroll.current,
      line = gap ? waiting.current : lines.current[primary];
    if (!container || !line || !lyrics.synced) {
      spring.current.velocity = 0;
      return;
    }
    let frame = 0,
      last = performance.now();
    spring.current.position = container.scrollTop;
    const tick = (time: number) => {
      frame = 0;
      if (!frameDue(last, time)) {
        frame = requestAnimationFrame(tick);
        return;
      }
      const motion = spring.current,
        slide = glide.current;
      if (slide) {
        const elapsed = time - slide.start;
        container.scrollTop = motion.position = glideScroll(
          slide.from,
          motion.target,
          elapsed,
        );
        motion.velocity = 0;
        last = time;
        if (elapsed < GLIDE_MS) frame = requestAnimationFrame(tick);
        else glide.current = undefined;
        return;
      }
      container.scrollTop = stepScroll(
        motion,
        Math.min(0.064, Math.max(0, (time - last) / 1000)),
      );
      last = time;
      if (
        Math.abs(motion.position - motion.target) > 0.2 ||
        Math.abs(motion.velocity) > 0.5
      )
        frame = requestAnimationFrame(tick);
      else container.scrollTop = motion.target;
    };
    const retarget = () => {
      const stacked = container
        .closest(".lyrics-page")
        ?.classList.contains("narrow");
      // Playback follows the line's centre, independent of artwork. Wide
      // lyrics still begin above centre and stop at the padded credits.
      // The Now playing card starts right under its header and keeps one
      // faded line above the current one, as Spicy Lyrics' card does.
      // Expanded over the whole panel, it reads like the lyrics page: the
      // song starts a third down and the current line stays centred.
      const card = compact && !container.closest(".lyric-preview.expanded");
      const focus = card
        ? Math.min(40, container.clientHeight * 0.14)
        : container.clientHeight / 2;
      const centred = !card;
      // Minimal lyrics in full screen begin lower, as in Spicy Lyrics.
      const opening =
        settings.lyrics_minimal && !compact && container.closest(".immersive")
          ? 0.4
          : 0.28;
      container.style.setProperty(
        "--lead-top",
        `${card ? 8 : compact || !stacked ? container.clientHeight * opening : Math.max(0, focus - (lines.current[0]?.offsetHeight ?? 0) / 2)}px`,
      );
      spring.current.target = Math.max(
        0,
        Math.min(
          container.scrollHeight - container.clientHeight,
          line.offsetTop + (centred ? line.offsetHeight / 2 : 0) - focus,
        ),
      );
      if (!following) {
        spring.current.velocity = 0;
        setReturnDirection(
          container.scrollTop > spring.current.target ? "up" : "down",
        );
        return;
      }
      if (reworded || !placed.current) {
        placed.current = true;
        reworded = false;
        cancelAnimationFrame(frame);
        frame = 0;
        glide.current = undefined;
        container.scrollTop = spring.current.position = spring.current.target;
        spring.current.velocity = 0;
        return;
      }
      // Smooth scrolling off: the browser's own smooth scroll, not the spring.
      const native = settings.lyrics_smooth_scroll === false;
      if (jumped.current && !settings.reduce_motion && !native)
        glide.current = {
          from: container.scrollTop,
          start: performance.now(),
        };
      if (settings.reduce_motion || native) {
        container.scrollTo({
          top: spring.current.target,
          behavior: native && !settings.reduce_motion ? "smooth" : "instant",
        });
        spring.current.position = spring.current.target;
        spring.current.velocity = 0;
      } else if (!frame) {
        last = performance.now();
        frame = requestAnimationFrame(tick);
      }
    };
    retarget();
    const observer = new ResizeObserver(retarget);
    observer.observe(container);
    observer.observe(line);
    container
      .querySelectorAll(".lyric-wait")
      .forEach((row) => observer.observe(row));
    const album = container
      .closest(".lyrics-page")
      ?.querySelector(".lyrics-album");
    if (album) observer.observe(album);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [
    primary,
    following,
    lyrics,
    settings.reduce_motion,
    settings.lyric_size,
    settings.romanized,
    settings.lyrics_translations,
    settings.lyrics_minimal,
    settings.lyrics_smooth_scroll,
    footerMode,
    gap?.start,
  ]);
  useLayoutEffect(() => {
    // Held until follow mode resumes, so a jump while browsing still glides.
    if (following) jumped.current = false;
  }, [positionMs, following]);
  useEffect(() => {
    if (following || !lyrics.synced) return;
    if (performance.now() - browsedAt.current < BROWSE_COOLDOWN_MS) return;
    const container = scroll.current,
      line = lines.current[primary];
    if (!container || !line) return;
    const view = container.getBoundingClientRect(),
      box = line.getBoundingClientRect();
    if (Math.min(box.bottom, view.bottom) - Math.max(box.top, view.top) >= 5)
      setFollowing(true);
  }, [positionMs, following, primary, lyrics.synced]);
  const highlighter = !!settings.lyrics_highlighter && lyrics.synced;
  if (lyrics.instrumental)
    return (
      <div className="lyrics-message">
        <Music2 />
        <h2>Instrumental</h2>
        <p>Enjoy the music</p>
        <LyricCredits
          lyrics={lyrics}
          discord={settings.lyrics_discord_usernames !== false}
          browser={settings.lyrics_profiles_in_browser}
        />
      </div>
    );
  return (
    <div
      className={`lyrics-display ${compact ? "compact" : ""} ${footerMode !== "off" ? "has-pinned-footer" : ""} ${!lyrics.synced ? "static-lyrics" : ""} ${settings.reduce_motion ? "reduce-motion" : ""} ${settings.lyrics_right_align ? "lyrics-right" : ""} ${settings.lyrics_flip_duet ? "lyrics-flip-duet" : ""} ${settings.lyrics_wide_duet === false ? "" : "lyrics-wide-duet"} ${settings.lyrics_line_hover ? "lyrics-line-hover" : ""} ${lyrics.lines.some((line) => line.opposite) ? "has-duet" : ""} ${settings.lyrics_minimal && !compact ? "minimal" : ""} ${highlighter ? "highlighter" : ""} ${settings.lyrics_keep_overlaps !== false ? "keep-overlaps" : ""} ${lyrics.synced && lyrics.lines.length && positionMs >= Math.max(...lyrics.lines.map(vocalEnd)) ? "all-sung" : ""} ${settings.lyrics_credits_follow && endsOnRight(lyrics.lines, !!(settings.lyrics_flip_duet || settings.lyrics_right_align)) ? "credits-right" : ""}`}
      data-following={following}
      style={
        {
          ...(compact
            ? {}
            : {
                "--lyric-size": `${Math.round(settings.lyric_size * 1.6)}px`,
              }),
          ...(highlighter
            ? {
                "--highlighter-ink": markerColour(
                  settings.lyrics_highlighter_color,
                ),
              }
            : {}),
        } as CSSProperties
      }
    >
      <div
        className="lyric-scroll"
        data-at-end={atEnd}
        onScroll={(event) => {
          const node = event.currentTarget;
          // Only the Now playing card fades its top by scroll distance. The
          // value stops changing once the fade is whole, so scrolling does
          // not restyle the lyrics on every frame.
          if (compact) {
            const scrolled = `${Math.min(80, Math.round(node.scrollTop))}px`;
            if (node.style.getPropertyValue("--scrolled") !== scrolled)
              node.style.setProperty("--scrolled", scrolled);
          }
          setAtEnd(
            node.scrollTop + node.clientHeight >= node.scrollHeight - 24,
          );
          if (!following)
            setReturnDirection(
              node.scrollTop > spring.current.target ? "up" : "down",
            );
        }}
        ref={scroll}
        onWheel={browse}
        onTouchStart={browse}
        tabIndex={0}
        onKeyDown={(event) => {
          if (
            [
              "ArrowUp",
              "ArrowDown",
              "PageUp",
              "PageDown",
              "Home",
              "End",
            ].includes(event.key)
          )
            browse();
        }}
      >
        <div className="lyric-spacer" />
        {gaps.has(-1) && waitingDots(gaps.get(-1)!)}
        {lyrics.lines.map((line, index) => {
          const playing = active.includes(index);
          const distance = Math.abs(index - primary);
          return (
            <Fragment key={`${line.start_ms}-${index}`}>
              <LyricRow
                line={line}
                action={lineActions[index]}
                playing={playing}
                past={!playing && positionMs >= vocalEnd(line)}
                blur={compact || playing ? 0 : lineBlur(distance)}
                synced={lyrics.synced}
                highlighter={highlighter}
                // A line keeps following the song until its last word ends,
                // even after the next line takes over, so the pen finishes
                // at the singing's pace; then it keeps all its ink. Lines
                // not being sung hold still, so they skip rendering.
                time={
                  !highlighter
                    ? 0
                    : playing ||
                        (positionMs >= line.start_ms &&
                          positionMs < vocalEnd(line))
                      ? positionMs
                      : positionMs >= line.start_ms
                        ? Infinity
                        : -Infinity
                }
                translation={settings.lyrics_translations}
              />
              {gaps.has(index) && waitingDots(gaps.get(index)!)}
            </Fragment>
          );
        })}
        {footerMode !== "full" && (
          <div className="lyric-end-credits">
            {writerCredits}
            {footerMode === "off" && (
              <LyricCredits
                lyrics={lyrics}
                expanded
                discord={settings.lyrics_discord_usernames !== false}
                browser={settings.lyrics_profiles_in_browser}
              />
            )}
          </div>
        )}
      </div>
      {footerMode !== "off" && (
        <div
          className="lyric-end-credits lyric-pinned-footer"
          role="region"
          aria-label="Pinned lyrics footer"
        >
          {footerMode === "full" && writerCredits}
          <LyricCredits
            lyrics={lyrics}
            expanded
            discord={settings.lyrics_discord_usernames !== false}
            browser={settings.lyrics_profiles_in_browser}
          />
        </div>
      )}
      {!following &&
        lyrics.synced &&
        settings.lyrics_follow_button !== false && (
          <button
            type="button"
            className="follow-lyrics"
            title="Return to current lyric"
            aria-label="Return to current lyric"
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => setFollowing(true)}
          >
            {returnDirection === "up" ? (
              <ChevronUp size={28} strokeWidth={2.5} />
            ) : (
              <ChevronDown size={28} strokeWidth={2.5} />
            )}
          </button>
        )}
    </div>
  );
}

const LyricCredits = memo(function LyricCredits({
  lyrics,
  expanded = false,
  discord = true,
  browser = false,
}: {
  lyrics: Lyrics;
  expanded?: boolean;
  /** Name people by Discord username when their profile gave one. */
  discord?: boolean;
  /** Open spicylyrics.org profiles in the browser instead of in the app. */
  browser?: boolean;
}) {
  const nameOf = (person: Contributor) =>
    (discord && person.discord) || person.username;
  const source =
    (
      {
        spicy_lyrics: "Spicy Lyrics",
        apple_music: "Apple Music",
        spotify: "Spotify",
        musixmatch: "Musixmatch",
      } as Record<string, string>
    )[lyrics.source] ?? lyrics.source;
  const openCreator = useCreatorProfile();
  const profileUrl = (url?: string) => {
    try {
      const parsed = new URL(url ?? "");
      if (
        parsed.protocol === "https:" &&
        parsed.hostname === "spicylyrics.org" &&
        !parsed.username &&
        !parsed.password
      )
        return parsed.href;
    } catch {
      /* Missing profile links remain plain credit text. */
    }
  };
  const community =
    lyrics.source === "spicy_lyrics"
      ? (["maker", "uploader"] as const).flatMap((role) => {
          const person = lyrics.attribution?.[role];
          return person?.username ? [{ role, person }] : [];
        })
      : [];
  // Community lyrics read like the Spicy Lyrics fork:
  // "Community lyrics · Made by (avatar) @name".
  if (community.length)
    return (
      <div
        className={`lyric-source community-credits ${expanded ? "expanded" : ""}`}
      >
        <span className="community-lead">Community lyrics</span>
        {community.map(({ role, person }) => {
          const url = profileUrl(person.url);
          const label =
            role === "uploader" && community[0].role === "maker"
              ? "Uploaded by"
              : "Made by";
          const name = (
            <>
              {person.avatar?.startsWith("https://") && (
                <img src={person.avatar} alt="" />
              )}
              <span className="community-name">@{nameOf(person)}</span>
            </>
          );
          return (
            <span key={role} className="community-person">
              <span className="community-dot"> · </span>
              {label}{" "}
              {person.id || url ? (
                <button
                  className="community-profile"
                  onClick={() =>
                    person.id && !(browser && url)
                      ? openCreator(person)
                      : void rpc("open_link", { url }).catch(() => {})
                  }
                >
                  {name}
                </button>
              ) : (
                <span className="community-profile">{name}</span>
              )}
            </span>
          );
        })}
      </div>
    );
  return (
    <div className={`lyric-source ${expanded ? "expanded" : ""}`}>
      Lyrics provided by: {source}
      {lyrics.estimated_words
        ? lyrics.estimated_lines
          ? " (+ estimated line and word timing)"
          : " (+ estimated word timing)"
        : lyrics.split_words && " (+ extra splits)"}
      {lyrics.source === "spicy_lyrics" &&
        (
          [
            ["uploader", "Uploaded by"],
            ["maker", "Made by"],
          ] as const
        ).map(([role, label]) => {
          const person = lyrics.attribution?.[role];
          if (!person?.username) return null;
          let url: string | undefined;
          try {
            const parsed = new URL(person.url ?? "");
            if (
              parsed.protocol === "https:" &&
              parsed.hostname === "spicylyrics.org" &&
              !parsed.username &&
              !parsed.password
            )
              url = parsed.href;
          } catch {
            /* Missing profile links remain plain credit text. */
          }
          return (
            <span key={role}>
              {" "}
              · {label}{" "}
              {person.id || url ? (
                <button
                  className="text-link"
                  onClick={() =>
                    person.id && !(browser && url)
                      ? openCreator(person)
                      : void rpc("open_link", { url }).catch(() => {})
                  }
                >
                  {expanded ? "@" : ""}
                  {nameOf(person)}
                </button>
              ) : (
                nameOf(person)
              )}
              {expanded && person.avatar?.startsWith("https://") && (
                <img className="credit-avatar" src={person.avatar} alt="" />
              )}
            </span>
          );
        })}
    </div>
  );
});

/** Static lyric markup. LyricAnimator moves the syllables and letters. */
/** How far either side of a pause its countdown follows song time. */
const COUNTDOWN_REACH_MS = 3000;

/** One lyric line. Memoised: while a song plays only the lines whose role,
 *  blur or ink changes render again, not every word of every line. */
const LyricRow = memo(function LyricRow({
  line,
  action,
  playing,
  past,
  blur,
  synced,
  highlighter,
  time,
  translation,
}: {
  line: Line;
  action: {
    ref: (element: HTMLButtonElement | null) => void;
    click: () => void;
  };
  playing: boolean;
  past: boolean;
  blur: number;
  synced: boolean;
  highlighter: boolean;
  time: number;
  translation?: boolean;
}) {
  return (
    <button
      ref={action.ref}
      className={`lyric-line ${playing ? "active" : ""} ${line.background ? "background-vocal" : ""} ${line.opposite ? "opposite" : ""} ${past ? "past" : ""}`}
      style={{ "--blur": `${blur}px` } as CSSProperties}
      onClick={action.click}
      disabled={!synced}
      // Clicking seeks without taking focus, so Space stays with play and
      // pause.
      onMouseDown={(event) => event.preventDefault()}
    >
      <span className="lyric-main">
        {highlighter ? (
          <HighlighterWords line={line} time={time} />
        ) : (
          <LyricWords line={line} />
        )}
      </span>
      {/* Lyrics can carry translations, such as Chinese under English; like
          Spicy Lyrics they stay hidden unless asked for. */}
      {translation && line.translation && (
        <span className="lyric-translation">{line.translation}</span>
      )}
    </button>
  );
});

const LyricWords = memo(function LyricWords({ line }: { line: Line }) {
  const groups = useMemo(() => wordGroups(line.words), [line]);
  if (!line.words.length)
    return <span className="lyric-plain">{line.text}</span>;
  return (
    <>
      {groups.map((group, index) => (
        <LyricWord key={index} words={group} />
      ))}
    </>
  );
});

function TimedLyricsPreview({
  state,
  onPlayer,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
}) {
  const current = usePosition(state) + state.settings.lyric_offset_ms;
  // Re-render when romanized lyrics finish converting.
  useRomanization();
  const lyrics = filterWords(
    displayLyrics(
      state.lyrics,
      !!state.settings.experimental_word_sync,
      state.settings.romanized,
      state.playback.item?.duration_ms,
    ),
    state.settings.lyrics_word_filter,
  );
  return lyrics ? (
    <LyricsDisplay
      lyrics={lyrics}
      writers={state.playback.item?.songwriters}
      positionMs={current}
      settings={state.settings}
      compact
      onSeek={(ms) => onPlayer({ command: "seek", positionMs: ms })}
    />
  ) : null;
}

const LYRICS_CARD_KEY = "lyssna.npv-lyrics-card";
type LyricsCardView = { open: boolean; expanded: boolean };
function savedCardView(): LyricsCardView {
  try {
    const saved = JSON.parse(localStorage.getItem(LYRICS_CARD_KEY) ?? "{}");
    return { open: saved.open !== false, expanded: saved.expanded === true };
  } catch {
    return { open: true, expanded: false };
  }
}
/** Spicy Lyrics' Now Playing lyrics card: open the lyrics page, expand the
 * card over the whole panel, or fold it down to its header. */
function LyricsCard({
  state,
  onPlayer,
  onLyrics,
  onExpand,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  onLyrics: () => void;
  onExpand: (expanded: boolean) => void;
}) {
  const [view, setView] = useState(savedCardView);
  const expanded = view.open && view.expanded;
  useEffect(() => {
    try {
      localStorage.setItem(LYRICS_CARD_KEY, JSON.stringify(view));
    } catch {
      // A remembered layout is a convenience; the card works without it.
    }
  }, [view]);
  useEffect(() => {
    onExpand(expanded);
    return () => onExpand(false);
  }, [expanded, onExpand]);
  return (
    <article
      className={`now-card lyric-preview ${view.open ? "" : "collapsed"} ${expanded ? "expanded" : ""}`}
    >
      <div className="card-heading">
        <span>Lyrics</span>
        <div className="lyric-card-controls">
          <button
            className="lyric-card-control"
            onClick={onLyrics}
            title="Open lyrics"
          >
            <PanelLeftOpen size={18} />
          </button>
          <button
            className="lyric-card-control"
            onClick={() =>
              // Expanding a folded card opens it in the same step.
              setView({ open: true, expanded: !expanded })
            }
            title={expanded ? "Exit expanded lyrics" : "Expand lyrics"}
            aria-pressed={expanded}
          >
            {expanded ? (
              <FoldVertical size={18} />
            ) : (
              <UnfoldVertical size={18} />
            )}
          </button>
          <button
            className="lyric-card-control lyric-card-toggle"
            onClick={() => setView({ open: !view.open, expanded: false })}
            title={view.open ? "Hide lyrics" : "Show lyrics"}
            aria-expanded={view.open}
          >
            <ChevronUp size={18} />
          </button>
        </div>
      </div>
      {view.open && <TimedLyricsPreview state={state} onPlayer={onPlayer} />}
    </article>
  );
}

type Links = {
  onAlbum: (album: Album) => void;
  onArtist: (artist: Artist) => void;
};
export { DJArtwork };
export function DJPresentation({ playing }: { playing: boolean }) {
  return (
    <div className="dj-presentation">
      <DJArtwork />
      <div>
        <h2>DJ</h2>
        <p>{playing ? "Speaking…" : "Paused"}</p>
      </div>
    </div>
  );
}
/** Lyric views narrower than this stack a small header above the lyrics.
 * Low enough that a 1440px laptop with Now playing open keeps the
 * side-by-side layout (about 730px). */
const NARROW_LYRICS_PX = 640;
function useNarrowLyrics() {
  const page = useRef<HTMLElement>(null);
  const [narrow, setNarrow] = useState(false);
  useLayoutEffect(() => {
    const element = page.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) =>
      setNarrow(entry.contentRect.width < NARROW_LYRICS_PX),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [page, narrow] as const;
}
function DJLyricsPage({
  state,
  children,
}: {
  state: AppState;
  children?: ReactNode;
}) {
  const [page, narrow] = useNarrowLyrics();
  return (
    <section
      ref={page}
      className={`lyrics-page dj-view ${narrow ? "narrow" : ""} ${state.settings.reduce_motion ? "reduce-motion" : ""}`}
    >
      {children}
      <Ambience
        image={DJ_COVER}
        reduceMotion={state.settings.reduce_motion}
        animate={state.playback.is_playing}
      />
      <div className="lyrics-album">
        <DJArtwork />
        <div className="lyrics-title">
          <span className="song-title">DJ</span>
        </div>
        <div className="lyrics-artists">
          {state.playback.is_playing ? (
            // Centre the word; the ellipsis hangs past it.
            <span className="hanging-ellipsis">
              Speaking<span aria-hidden="true">…</span>
            </span>
          ) : (
            "Paused"
          )}
        </div>
      </div>
      <div className="lyrics-body dj-lyrics-message" role="status">
        <h2>Lyrics aren't available while the DJ talks</h2>
        <p>
          Your DJ is setting up what plays next. Lyrics return when the next
          song starts.
        </p>
      </div>
    </section>
  );
}
export function LyricsPage(props: Parameters<typeof SongLyricsPage>[0]) {
  return props.state.narrating ? (
    <DJLyricsPage state={props.state}>{props.children}</DJLyricsPage>
  ) : (
    <SongLyricsPage {...props} />
  );
}
function SongLyricsPage({
  state,
  onPlayer,
  onAlbum,
  onArtist,
  onImport,
  onRetry,
  onSave,
  onCanvasActive,
  immersive = false,
  children,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  onImport: () => void;
  onRetry: () => void;
  onSave: (track: Track) => void;
  onCanvasActive?: (active: boolean) => void;
  immersive?: boolean;
  children?: ReactNode;
} & Links) {
  const songTime = usePosition(state),
    current = songTime + state.settings.lyric_offset_ms,
    track = state.playback.item;
  // Full screen seek: preview while dragging, seek once on release.
  const [seeking, setSeeking] = useState<number | null>(null);
  // After release the bar holds the chosen spot until playback reports it,
  // instead of flicking back to the old time for a moment.
  const [landing, setLanding] = useState<{ to: number; at: number } | null>(
    null,
  );
  const landed =
    !landing ||
    Math.abs(songTime - landing.to) < 750 ||
    performance.now() - landing.at > 2000;
  useEffect(() => {
    if (landing && landed) setLanding(null);
  }, [landing, landed]);
  const seekShown = seeking ?? (landed ? songTime : landing!.to);
  const commitSeek = () => {
    if (seeking === null) return;
    onPlayer({ command: "seek", positionMs: seeking });
    setLanding({ to: seeking, at: performance.now() });
    setSeeking(null);
    wakeTools();
  };
  // This page renders every frame; stable click handlers stop React from
  // rewriting them each time.
  const playing = state.playback.is_playing;
  const previousSong = useCallback(
    () => onPlayer({ command: "previous" }),
    [onPlayer],
  );
  const nextSong = useCallback(() => onPlayer({ command: "next" }), [onPlayer]);
  const togglePlay = useCallback(
    () => onPlayer({ command: playing ? "pause" : "play" }),
    [onPlayer, playing],
  );
  const shuffle = state.playback.shuffle_state;
  const toggleShuffle = useCallback(
    () => onPlayer({ command: "shuffle", enabled: !shuffle }),
    [onPlayer, shuffle],
  );
  const repeat = state.playback.repeat_state || "off";
  const cycleRepeat = useCallback(
    () =>
      onPlayer({
        command: "repeat",
        repeat:
          repeat === "off" ? "context" : repeat === "context" ? "track" : "off",
      }),
    [onPlayer, repeat],
  );
  const saved = !!(track && state.saved[track.uri]);
  const toggleSaved = useCallback(() => {
    if (track) onSave(track);
  }, [onSave, track]);
  const backdrop = useRef<HTMLCanvasElement>(null);
  const [page, measuredNarrow] = useNarrowLyrics();
  // The toolbar shows while the pointer moves over the lyrics and fades a
  // second after it stops. Kept as a data attribute so frequent re-renders
  // of this page do not reset it.
  const toolsTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const wakeTools = () => {
    const element = page.current;
    if (!element) return;
    element.dataset.tools = "awake";
    clearTimeout(toolsTimer.current);
    toolsTimer.current = setTimeout(() => {
      delete element.dataset.tools;
    }, 750);
  };
  const sleepTools = () => {
    clearTimeout(toolsTimer.current);
    if (page.current) delete page.current.dataset.tools;
  };
  useEffect(() => () => clearTimeout(toolsTimer.current), []);
  // The toolbar's compact toggle forces the stacked layout.
  const narrow = measuredNarrow || !!state.settings.lyrics_compact;
  const [canvasShown, setCanvasShown] = useState(false);
  const motionAllowed =
    !state.settings.reduce_motion &&
    !matchMedia("(prefers-reduced-motion: reduce)").matches;
  const canvas = !!(
    track &&
    state.canvas_url &&
    state.settings.canvas_lyrics &&
    motionAllowed
  );
  const visibleCanvas =
    canvas && canvasShown && !state.settings.lyrics_hide_artwork;
  useEffect(() => {
    onCanvasActive?.(visibleCanvas);
    return () => onCanvasActive?.(false);
  }, [visibleCanvas, onCanvasActive]);
  const coverColour =
    useArtworkColor(imageOf(track?.album.images)) ?? "rgb(40 40 40)";
  // Space Gravity needs word timing, including estimates for static lyrics.
  const gravity = !!(
    state.settings.space_gravity &&
    state.lyrics &&
    !state.lyrics.instrumental &&
    (state.lyrics.synced || track?.duration_ms)
  );
  // Re-render when romanized lyrics finish converting.
  useRomanization();
  const lyrics = filterWords(
    displayLyrics(
      state.lyrics,
      !!state.settings.experimental_word_sync || gravity,
      state.settings.romanized,
      track?.duration_ms,
    ),
    state.settings.lyrics_word_filter,
  );
  const year = state.settings.lyrics_year_position ?? "left";
  // Cover art and Artist header put a plain image behind the lyrics; the
  // artist's header falls back to the cover until it is known.
  const background = state.settings.lyrics_background ?? "animated";
  const cover = artworkFor(track?.album.images, 1280);
  const backgroundImage =
    background === "cover"
      ? cover
      : background === "artist"
        ? [
            state.details.artists.find(
              (artist) => artist.id && artist.id === track?.artists[0]?.id,
            )?.header_image ?? "",
          ].find(usableImage) || cover
        : undefined;
  // Both time labels reserve the duration's width, so the seek bar between
  // them keeps its length as the elapsed time changes.
  const timeChars = {
    "--time-chars": track ? formatTime(track.duration_ms).length : 4,
  } as CSSProperties;
  const seekPart = track ? (
    <>
      {/* A frosted glass track with a white fill; a transparent range
          on top handles dragging and the keyboard. */}
      <div
        className="lyrics-glass-seek"
        style={
          {
            "--range": `${(seekShown / Math.max(1, track.duration_ms || 1)) * 100}%`,
          } as CSSProperties
        }
      >
        <span className="lyrics-glass-fill" />
        <input
          type="range"
          aria-label="Seek position"
          min="0"
          max={track.duration_ms || 1}
          value={Math.min(seekShown, track.duration_ms || 1)}
          onChange={(event) => setSeeking(Number(event.target.value))}
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId);
            setSeeking(songTime);
            wakeTools();
          }}
          // Releasing the pointer lets go of focus, which commits once. A
          // focused slider would otherwise show its keyboard ring as soon as
          // Space toggles playback.
          onPointerUp={(event) => {
            if (document.activeElement === event.currentTarget)
              event.currentTarget.blur();
            else commitSeek();
          }}
          onKeyUp={commitSeek}
          onPointerCancel={() => setSeeking(null)}
          onBlur={commitSeek}
        />
      </div>
      <div className="lyrics-fullscreen-times">
        <time>{formatTime(seekShown)}</time>
        <time>{formatTime(track.duration_ms)}</time>
      </div>
    </>
  ) : null;
  const heartPart = track ? (
    <button
      className={`icon-button lyrics-fullscreen-like ${saved ? "is-saved" : ""}`}
      title={saved ? "Remove from Liked Songs" : "Save to Liked Songs"}
      onClick={toggleSaved}
    >
      <FullscreenHeartIcon />
    </button>
  ) : null;
  const transportPart = track ? (
    <>
      <button
        className={`icon-button lyrics-fullscreen-shuffle ${state.playback.shuffle_state ? "active" : ""}`}
        title="Shuffle"
        onClick={toggleShuffle}
      >
        <FullscreenShuffleIcon />
      </button>
      <button
        className="icon-button lyrics-fullscreen-skip"
        title="Previous song"
        onClick={previousSong}
      >
        <SkipPreviousIcon />
      </button>
      <button
        className="icon-button lyrics-fullscreen-play"
        aria-label={state.playback.is_playing ? "Pause" : "Play"}
        onClick={togglePlay}
      >
        {state.playback.is_playing ? (
          <FullscreenPauseIcon />
        ) : (
          <FullscreenPlayIcon />
        )}
      </button>
      <button
        className="icon-button lyrics-fullscreen-skip"
        title="Next song"
        onClick={nextSong}
      >
        <SkipNextIcon />
      </button>
      <button
        className={`icon-button lyrics-fullscreen-repeat ${repeat !== "off" ? "active" : ""}`}
        title="Repeat"
        onClick={cycleRepeat}
      >
        <FullscreenRepeatIcon one={repeat === "track"} />
      </button>
    </>
  ) : null;
  const volumePart = track ? (
    <>
      {state.settings.fullscreen_volume &&
        state.settings.fullscreen_volume !== "off" && (
          <FullscreenVolume
            state={state}
            onPlayer={onPlayer}
            side={state.settings.fullscreen_volume}
          />
        )}
    </>
  ) : null;
  // Over the cover, parts kept up by Always show in full screen, and all of
  // them when the song has no lyrics, sit outside the artwork as the fork's
  // do: the seek bar under the cover, the transport under the song details.
  const coverLayout =
    immersive && !canvas && !narrow && !state.settings.lyrics_hide_artwork;
  const noLyrics = !lyrics && !!state.lyric_error;
  const alwaysShow = state.settings.fullscreen_always_show ?? "none";
  const seekOutside =
    coverLayout && (noLyrics || alwaysShow === "time" || alwaysShow === "both");
  const transportOutside =
    coverLayout &&
    (noLyrics || alwaysShow === "controls" || alwaysShow === "both");
  const fullscreenControls = track ? (
    <div className="lyrics-fullscreen-controls" style={timeChars}>
      {!seekOutside && seekPart}
      <div className="lyrics-fullscreen-buttons">
        {heartPart}
        {!transportOutside && transportPart}
      </div>
      {volumePart}
    </div>
  ) : null;
  return (
    <section
      ref={page}
      className={`lyrics-page ${canvas ? "canvas-lyrics" : ""} ${narrow ? "narrow" : ""} ${state.settings.reduce_motion ? "reduce-motion" : ""} ${gravity ? "space-gravity" : ""} ${state.settings.lyrics_hide_artwork ? "no-artwork" : ""} ${state.settings.lyrics_artwork_right ? "artwork-right" : ""} ${state.settings.lyrics_background === "color" ? "colour-background" : ""} ${state.settings.lyrics_toolbar_bottom ? "toolbar-bottom" : ""} ${state.settings.glass_seek === false ? "flat-seek" : ""} ${coverLayout && noLyrics ? "no-lyrics" : ""}`}
      style={
        {
          // Shared by the lyric lines and, in Space Gravity, its countdown.
          "--lyric-size": `${Math.round(state.settings.lyric_size * 1.6)}px`,
          "--lyrics-cover-color": coverColour,
          ...(state.settings.lyrics_background === "color"
            ? { "--hero": coverColour }
            : {}),
        } as CSSProperties
      }
      data-seeking={seeking !== null || undefined}
      data-always-show={
        state.settings.fullscreen_always_show &&
        state.settings.fullscreen_always_show !== "none"
          ? state.settings.fullscreen_always_show
          : undefined
      }
      onMouseMove={wakeTools}
      onKeyDownCapture={wakeTools}
      onMouseLeave={sleepTools}
    >
      {(!immersive || canvas || narrow || state.settings.lyrics_hide_artwork) &&
        children}
      {(!canvas || !canvasShown) &&
        (background === "animated" || background === "static") && (
          <Ambience
            track={track}
            reduceMotion={state.settings.reduce_motion}
            animate={state.playback.is_playing && background !== "static"}
          />
        )}
      {(!canvas || !canvasShown) && backgroundImage && (
        <div
          className="lyrics-image-background"
          aria-hidden="true"
          style={
            {
              backgroundImage: `url("${backgroundImage.replace(/"/g, "%22")}")`,
              "--background-blur": `${state.settings.lyrics_background_blur ?? 0}px`,
            } as CSSProperties
          }
        />
      )}
      {canvas && background !== "none" && (
        <canvas
          className="lyrics-canvas-backdrop"
          ref={backdrop}
          width={128}
          height={128}
          aria-hidden="true"
        />
      )}
      {track ? (
        <>
          <div
            className="lyrics-album"
            // The Canvas sits behind this box as a backdrop, so only the
            // title and artists open the song menu over it.
            data-context-route={
              track.id && !canvas ? `track:${track.id}` : undefined
            }
          >
            {canvas ? (
              <LyricsCanvas
                key={track.uri}
                url={state.canvas_url!}
                poster={imageOf(track.album.images)}
                playing={state.playback.is_playing}
                backdrop={backdrop}
                onActive={setCanvasShown}
              />
            ) : (
              <div className="lyrics-media">
                <Cover
                  images={track.album.images}
                  fade={coverFade(state.settings)}
                  onClick={() => onAlbum(track.album)}
                />
                {immersive &&
                  !narrow &&
                  !state.settings.lyrics_hide_artwork &&
                  children}
                {!narrow &&
                  !state.settings.lyrics_hide_artwork &&
                  fullscreenControls}
              </div>
            )}
            {seekOutside && (
              <div
                className="lyrics-fullscreen-controls lyrics-outside-controls outside-seek"
                style={timeChars}
              >
                {seekPart}
              </div>
            )}
            <div
              className="lyrics-details"
              data-context-route={track.id ? `track:${track.id}` : undefined}
            >
              <TrackTitle
                track={track}
                onAlbum={onAlbum}
                reduceMotion={state.settings.reduce_motion}
                className="lyrics-title"
              />
              {/* The year stays put; only the artist names scroll. */}
              <div className="lyrics-artists">
                {track.album.release_date && year === "left" && (
                  <span className="lyrics-year">
                    {track.album.release_date.slice(0, 4)}
                    {/* Spaced like the artist's own inset on the far side. */}
                    <span className="lyrics-year-dot">·</span>
                  </span>
                )}
                <Marquee
                  text={artistNames(track)}
                  reduceMotion={state.settings.reduce_motion}
                >
                  <ArtistLinks artists={track.artists} onArtist={onArtist} />
                </Marquee>
                {track.album.release_date && year === "right" && (
                  <span className="lyrics-year right">
                    <span className="lyrics-year-dot">·</span>
                    {track.album.release_date.slice(0, 4)}
                  </span>
                )}
              </div>
            </div>
            {transportOutside && (
              <div className="lyrics-fullscreen-controls lyrics-outside-controls outside-transport">
                <div className="lyrics-fullscreen-buttons">{transportPart}</div>
              </div>
            )}
            {(canvas || narrow || state.settings.lyrics_hide_artwork) &&
              fullscreenControls}
          </div>
          <div className="lyrics-body">
            {lyrics && gravity ? (
              <SpaceGravityLyrics
                lyrics={lyrics}
                time={current}
                reduceMotion={!motionAllowed}
                tuning={state.settings.lyrics_animator}
                highlighter={
                  state.settings.lyrics_highlighter && lyrics.synced
                    ? markerColour(state.settings.lyrics_highlighter_color)
                    : undefined
                }
                linkWidth={
                  state.settings.gravity_links === false
                    ? 0
                    : (state.settings.gravity_link_width ?? 100) / 100
                }
                onSeek={(positionMs) =>
                  onPlayer({
                    command: "seek",
                    positionMs:
                      state.settings.lyrics_seek_compensation !== false
                        ? lyricSeekTarget(positionMs)
                        : positionMs,
                  })
                }
                credits={
                  <>
                    <WriterCredits
                      names={
                        lyrics.songwriters?.length
                          ? lyrics.songwriters
                          : (track.songwriters ?? [])
                      }
                    />
                    <LyricCredits
                      lyrics={lyrics}
                      expanded
                      discord={
                        state.settings.lyrics_discord_usernames !== false
                      }
                      browser={state.settings.lyrics_profiles_in_browser}
                    />
                  </>
                }
              />
            ) : lyrics ? (
              <LyricsDisplay
                lyrics={lyrics}
                writers={state.playback.item?.songwriters}
                positionMs={current}
                settings={state.settings}
                onSeek={(ms) => onPlayer({ command: "seek", positionMs: ms })}
              />
            ) : !state.lyric_error &&
              state.settings.lyrics_skeleton !== false ? (
              <LyricsSkeleton />
            ) : state.lyric_error === NO_LYRICS &&
              state.settings.lyrics_no_lyrics_notice === false ? null : (
              <div className="lyrics-message">
                <Mic2 />
                <h2>
                  {state.lyric_error ? "Lyrics unavailable" : "Finding lyrics…"}
                </h2>
                <p>
                  {state.lyric_error
                    ? "Try another lyrics source or import your lyric file."
                    : "Synced lyrics will appear here."}
                </p>
                {state.lyric_error && (
                  <div className="inline-actions">
                    <button className="pill" onClick={onRetry}>
                      <RotateCw size={16} />
                      Retry
                    </button>
                    <button className="pill" onClick={onImport}>
                      <Upload size={16} />
                      Import lyrics
                    </button>
                  </div>
                )}
              </div>
            )}
          </div>
        </>
      ) : (
        <div className="lyrics-message">
          <Music2 />
          <h2>Play something you love</h2>
          <p>Your lyrics will appear here.</p>
        </div>
      )}
    </section>
  );
}

/** The backend's answer when every source had no lyrics (not a failure). */
const NO_LYRICS = "No lyrics found for this song";
/** Ragged placeholder widths, fixed so the lines never reshuffle. */
const SKELETON_WIDTHS = [71, 56, 87, 48, 76, 63, 90, 52, 69, 81, 59, 74];
/** Placeholder lines where the lyrics will land, while they are fetched. */
function LyricsSkeleton() {
  return (
    <div className="lyrics-skeleton" role="status">
      <div className="lyrics-skeleton-lines" aria-hidden="true">
        {SKELETON_WIDTHS.map((width, index) => (
          <span
            key={index}
            style={{ width: `${width}%`, "--line": index } as CSSProperties}
          />
        ))}
      </div>
      <p>
        Loading lyrics
        <span className="lyrics-skeleton-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
      </p>
    </div>
  );
}

/** Volume beside or below the full screen controls, as the fork's
 *  Fullscreen Volume Slider. */
function FullscreenVolume({
  state,
  onPlayer,
  side,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  side: "left" | "right" | "below";
}) {
  const device = state.playback.device;
  const [volume, setVolume] = useState(
    device.volume_percent ?? state.settings.volume_percent ?? 100,
  );
  const send = useRef(onPlayer);
  send.current = onPlayer;
  const control = useMemo(
    () =>
      new VolumeControl(
        (value) => send.current({ command: "volume", volume: value }),
        setVolume,
      ),
    [],
  );
  useEffect(() => () => control.dispose(), [control]);
  useEffect(() => {
    control.observe(
      device.volume_percent ??
        (state.settings.soloist ? state.settings.volume_percent : undefined),
      device.id ?? device.name,
    );
  }, [
    device.volume_percent,
    device.id,
    device.name,
    state.settings.soloist,
    state.settings.volume_percent,
    control,
  ]);
  // The level a tap on the speaker brings back after muting.
  const audible = useRef(volume || 50);
  if (volume) audible.current = volume;
  // The wheel nudges the volume 5% a notch; React's wheel listener is
  // passive, so the page would scroll too.
  const bar = useRef<HTMLDivElement>(null);
  const level = useRef(volume);
  level.current = volume;
  useEffect(() => {
    const node = bar.current;
    if (!node) return;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      const next = Math.max(
        0,
        Math.min(100, level.current + (event.deltaY < 0 ? 5 : -5)),
      );
      control.edit(next);
      control.finish();
    };
    node.addEventListener("wheel", wheel, { passive: false });
    return () => node.removeEventListener("wheel", wheel);
  }, [control]);
  // Like the fork's: a frosted pill filled white from the bottom (or the
  // left, below the controls), its speaker inside the end of the fill,
  // growing wider while used.
  return (
    <div className={`lyrics-fullscreen-volume ${side}`}>
      <div
        ref={bar}
        className="lyrics-volume-pill"
        data-level={volume <= 0 ? "muted" : volume < 50 ? "low" : "high"}
        style={{ "--range": `${volume}%` } as CSSProperties}
      >
        <span className="lyrics-volume-fill" />
        <input
          type="range"
          aria-label="Full screen volume"
          min="0"
          max="100"
          value={volume}
          onPointerDown={() => control.begin()}
          onPointerUp={(event) => {
            control.finish();
            event.currentTarget.blur();
          }}
          onPointerCancel={() => control.finish()}
          onKeyUp={() => control.finish()}
          onBlur={() => control.finish()}
          onChange={(event) => control.edit(Number(event.target.value))}
        />
        <button
          className="lyrics-volume-icon"
          title={volume ? "Mute" : "Unmute"}
          onClick={() => {
            control.edit(volume ? 0 : audible.current);
            control.finish();
          }}
        >
          <FullscreenVolumeIcon />
        </button>
      </div>
    </div>
  );
}

// Videos warmed before playback so a Canvas starts at once instead of
// showing its still poster while it downloads.
const warmCanvases = new Map<string, HTMLVideoElement>();
export function preloadCanvas(url: string) {
  if (!url.startsWith("https://") || warmCanvases.has(url)) return;
  if (warmCanvases.size >= 6) {
    const [oldest, video] = warmCanvases.entries().next().value!;
    video.removeAttribute("src");
    video.load();
    warmCanvases.delete(oldest);
  }
  const video = document.createElement("video");
  video.muted = true;
  video.preload = "auto";
  video.src = url;
  video.load();
  warmCanvases.set(url, video);
}
const canvasRequests = new Set<string>();
export function prefetchCanvas(uri: string) {
  if (canvasRequests.has(uri)) return;
  canvasRequests.add(uri);
  void rpc("prefetch_canvas", { uris: [uri] }).catch(() =>
    canvasRequests.delete(uri),
  );
}

function CanvasArtwork({
  track,
  url,
  playing,
  reduceMotion,
  onOpen,
}: {
  track: Track;
  url?: string;
  playing: boolean;
  reduceMotion: boolean;
  onOpen: () => void;
}) {
  const video = useRef<HTMLVideoElement>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const element = video.current;
    if (!element) return;
    let visible = true;
    const update = () => {
      if (playing && visible && !document.hidden && !reduceMotion)
        void element.play().catch(() => {});
      else element.pause();
    };
    const observer = new IntersectionObserver((entries) => {
      visible = entries[0]?.isIntersecting ?? false;
      update();
    });
    observer.observe(element);
    document.addEventListener("visibilitychange", update);
    update();
    return () => {
      observer.disconnect();
      document.removeEventListener("visibilitychange", update);
      element.pause();
    };
  }, [url, playing, reduceMotion]);
  if (!url || failed || reduceMotion)
    return (
      <Cover
        images={track.album.images}
        className="now-artwork"
        onClick={onOpen}
      />
    );
  return (
    <button className="canvas-artwork" aria-label="Open album" onClick={onOpen}>
      <video
        ref={video}
        data-canvas-video
        crossOrigin="anonymous"
        src={url}
        muted
        loop
        playsInline
        autoPlay={playing && !reduceMotion}
        preload="auto"
        poster={imageOf(track.album.images)}
        onError={() => setFailed(true)}
      />
    </button>
  );
}

export function NowPlaying({
  state,
  onPlayer,
  onAlbum,
  onArtist,
  onLyrics,
  onQueue,
  onFollow,
  onSave,
  showLyricsPreview,
  canvasInLyrics = false,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  onLyrics: () => void;
  onQueue: () => void;
  onFollow: (artist: Artist) => void;
  onSave: (track: Track) => void;
  showLyricsPreview: boolean;
  canvasInLyrics?: boolean;
} & Links) {
  const track = state.playback.item;
  // Local songs have no Spotify artist, so no About the artist card.
  const listed = track?.is_local
    ? undefined
    : (state.details.artists[0] ?? track?.artists[0]);
  // About details arrive by artist, often before the song's details, and
  // for upcoming songs ahead of time.
  const about = listed?.id ? state.artist_about[listed.id] : undefined;
  const artist: Artist | undefined =
    listed && about && !listed.about_loaded
      ? {
          ...listed,
          about_loaded: true,
          gallery_image: about.gallery_image || listed.gallery_image,
          header_image: about.header_image || listed.header_image,
          monthly_listeners:
            about.monthly_listeners ?? listed.monthly_listeners,
          verified: about.verified ?? listed.verified,
          biography: about.biography || listed.biography,
          images:
            listed.images?.length || !about.image
              ? listed.images
              : [{ url: about.image }],
        }
      : listed;
  const next = state.details.queue[0];
  const saved = track && state.saved[track.uri];
  const [lyricsExpanded, setLyricsExpanded] = useState(false);
  if (state.narrating)
    return (
      <aside className="now-playing dj-view">
        <Ambience
          image={DJ_COVER}
          reduceMotion={state.settings.reduce_motion}
          animate={state.playback.is_playing}
        />
        <div className="now-playing-scroll">
          <div className="section-heading">
            <h2>Now playing</h2>
          </div>
          <DJArtwork />
          <div className="now-track">
            <div className="now-track-text">
              <span className="song-title">DJ</span>
              <div className="now-artists">
                {state.playback.is_playing ? "Speaking…" : "Paused"}
              </div>
            </div>
          </div>
          <p className="dj-note">
            Your DJ picks music for you and talks between sets. Song details
            return when the next track starts.
          </p>
        </div>
      </aside>
    );
  // Spotify's Canvas layout: the video fills the panel top edge to edge with
  // the header and title laid over it.
  const canvas =
    !!track &&
    !!state.canvas_url &&
    !state.settings.reduce_motion &&
    !canvasInLyrics;
  const heading = (
    <div className={`section-heading ${canvas ? "canvas-hero-head" : ""}`}>
      <h2>{canvas && track ? track.name : "Now playing"}</h2>
      <button className="icon-button" onClick={onLyrics} title="Open lyrics">
        <Maximize2 size={17} />
      </button>
    </div>
  );
  return (
    <aside
      className={`now-playing ${canvas ? "has-canvas" : ""} ${lyricsExpanded ? "lyrics-expanded" : ""}`}
    >
      {state.settings.npv_ambience !== false && (
        <Ambience
          track={track}
          reduceMotion={state.settings.reduce_motion}
          animate={state.playback.is_playing}
          // Only while Now playing shows the Canvas itself; with it on the
          // lyrics page, the panel keeps the album artwork's ambience.
          followCanvas={canvas}
        />
      )}
      <div className="now-playing-scroll">
        {!canvas && heading}
        {track ? (
          <>
            {canvas ? (
              <div className="canvas-hero">
                <CanvasArtwork
                  key={track.uri}
                  track={track}
                  url={state.canvas_url}
                  playing={state.playback.is_playing}
                  reduceMotion={state.settings.reduce_motion}
                  onOpen={() => onAlbum(track.album)}
                />
                <div className="canvas-hero-shade" />
                {heading}
              </div>
            ) : (
              <CanvasArtwork
                key={track.uri}
                track={track}
                url={canvasInLyrics ? undefined : state.canvas_url}
                playing={state.playback.is_playing}
                reduceMotion={state.settings.reduce_motion}
                onOpen={() => onAlbum(track.album)}
              />
            )}
            <div
              className={`now-track ${canvas ? "canvas-hero-track" : ""}`}
              data-context-route={track.id ? `track:${track.id}` : undefined}
            >
              <div className="now-track-text">
                <TrackTitle
                  track={track}
                  onAlbum={onAlbum}
                  reduceMotion={state.settings.reduce_motion}
                />
                <Marquee
                  text={artistNames(track)}
                  reduceMotion={state.settings.reduce_motion}
                  className="now-artists"
                >
                  <ExplicitBadge track={track} />
                  <ArtistLinks artists={track.artists} onArtist={onArtist} />
                </Marquee>
              </div>
              <button
                className={`icon-button save-track ${saved ? "is-saved" : ""}`}
                title={saved ? "Saved to Liked Songs" : "Save to Liked Songs"}
                onClick={() => onSave(track)}
              >
                {saved ? (
                  <CircleCheck size={24} fill="currentColor" stroke="#000" />
                ) : (
                  <CirclePlus size={24} />
                )}
              </button>
            </div>
            {showLyricsPreview && state.lyrics && (
              <LyricsCard
                state={state}
                onPlayer={onPlayer}
                onLyrics={onLyrics}
                onExpand={setLyricsExpanded}
              />
            )}
            {artist && (
              <article className="now-card artist-card">
                {(() => {
                  const photo = artist.gallery_image || imageOf(artist.images);
                  const waiting = !artist.about_loaded;
                  return (
                    // Name, listeners and Follow sit on the photo, as on
                    // Spotify's About section, so the card has no empty band.
                    <div
                      className={`artist-portrait ${photo || waiting ? "" : "no-photo"}`}
                    >
                      {(photo || waiting) && (
                        <button
                          className="artist-portrait-open"
                          aria-label={`Open ${artist.name}`}
                          onClick={() => onArtist(artist)}
                        >
                          {/* Spotify's card shows the About section's photo;
                              until it is known, nothing, so the avatar never
                              flashes. */}
                          {!waiting && <img src={photo} alt={artist.name} />}
                        </button>
                      )}
                      <strong>About the artist</strong>
                      <div className="artist-card-row">
                        <div className="artist-card-who">
                          <button
                            className="text-link artist-card-name"
                            onClick={() => onArtist(artist)}
                          >
                            <span className="artist-card-name-text">
                              {artist.name}
                            </span>
                            {artist.verified && (
                              <VerifiedBadge label="Verified artist" />
                            )}
                          </button>
                          <p className="artist-stat">
                            {artist.monthly_listeners != null
                              ? `${artist.monthly_listeners.toLocaleString()} monthly listeners`
                              : artist.followers
                                ? `${artist.followers.total.toLocaleString()} followers`
                                : ""}
                          </p>
                        </div>
                        <button
                          className="pill outline"
                          onClick={() => onFollow(artist)}
                        >
                          {state.saved[
                            artist.uri ?? `spotify:artist:${artist.id}`
                          ]
                            ? "Following"
                            : state.can_follow
                              ? "Follow"
                              : "Enable following"}
                        </button>
                      </div>
                    </div>
                  );
                })()}
                {artist.biography ? (
                  <div className="card-content artist-card-about">
                    <p className="artist-bio">{artist.biography}</p>
                  </div>
                ) : (
                  !!artist.genres?.length && (
                    <div className="card-content artist-card-about">
                      <p className="artist-genres">
                        {artist.genres.slice(0, 3).join(" · ")}
                      </p>
                    </div>
                  )
                )}
              </article>
            )}
            <CreditsCard
              track={track}
              saved={state.saved}
              onArtist={onArtist}
              onFollow={onFollow}
            />
            {next && (
              <article className="now-card queue-card">
                <div className="card-heading">
                  <span>Next in queue</span>
                  <button className="text-link muted" onClick={onQueue}>
                    Open queue
                  </button>
                </div>
                <button
                  className="next-track"
                  onClick={() =>
                    onPlayer({ command: "play", uri: next.uri, track: next })
                  }
                >
                  <Cover images={next.album.images} />
                  <div>
                    <Marquee
                      text={next.name}
                      reduceMotion={state.settings.reduce_motion}
                    >
                      {next.name}
                    </Marquee>
                    <span>{artistNames(next)}</span>
                  </div>
                </button>
              </article>
            )}
            <div className="playing-on">
              <span className={state.native_ready ? "ready-dot" : ""} />
              <div>
                <small>Playing on</small>
                <strong>
                  {state.playback.device.name.startsWith("Lyssna")
                    ? "This computer"
                    : state.playback.device.name || "This computer"}
                </strong>
              </div>
            </div>
          </>
        ) : (
          <div className="empty-now">
            <Music2 />
            <h3>Nothing playing yet</h3>
            <p>Pick a song to make yourself at home.</p>
          </div>
        )}
      </div>
    </aside>
  );
}

/** The play queue, shown in the right panel in place of Now playing. */
/** The Jam this computer hosts or joined, above the queue as in Spotify. */
function JamCard({
  jam,
  invite,
  hostName,
  onInvite,
  onCopy,
  onEnd,
  onControl,
}: {
  jam: Jam;
  invite: boolean;
  hostName: string;
  onInvite: (open: boolean) => void;
  onCopy: () => void;
  onEnd: () => void;
  onControl: (allowed: boolean) => void;
}) {
  const host =
    jam.members.find((member) => member.id === jam.owner_id) ?? jam.members[0];
  const name = host?.display_name || host?.username || hostName;
  return (
    <section className="jam-card" aria-label="Jam">
      <h2>{name ? `${name}\u2019s Jam` : "Jam"}</h2>
      <div className="jam-actions">
        <div
          className="jam-members"
          title={`${jam.members.length} in this Jam`}
        >
          {jam.members.slice(0, 3).map((member) =>
            member.image_url ? (
              <img key={member.id} src={member.image_url} alt="" />
            ) : (
              <span key={member.id} className="jam-member-initial">
                {(member.display_name || member.username).slice(0, 1)}
              </span>
            ),
          )}
          {jam.members.length > 3 && (
            <span className="jam-member-more">+{jam.members.length - 3}</span>
          )}
        </div>
        <button className="jam-pill" onClick={() => onInvite(true)}>
          Invite
        </button>
        <button className="jam-pill" onClick={onEnd}>
          {jam.is_owner ? "End" : "Leave"}
        </button>
      </div>
      {invite && (
        <div className="jam-invite">
          <div className="jam-invite-head">
            <div>
              <strong>
                Invite friends to {jam.is_owner ? "your" : "this"} Jam
              </strong>
              <p>
                Copy and share the link with your friends, or ask them to scan
                the QR code.
              </p>
            </div>
            <button
              className="icon-button"
              onClick={() => onInvite(false)}
              aria-label="Close invite"
            >
              <X size={18} />
            </button>
          </div>
          <div className="jam-invite-body">
            <button className="jam-copy" onClick={onCopy}>
              <Copy size={18} />
              Copy link
            </button>
            {jam.qr_svg && (
              <img
                className="jam-qr"
                alt="Jam QR code"
                src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(jam.qr_svg)}`}
              />
            )}
          </div>
        </div>
      )}
      {jam.is_owner && (
        <label className="jam-control">
          <span>Let others change what's playing</span>
          <Toggle
            label="Let others change what's playing"
            checked={!jam.queue_only}
            onChange={onControl}
          />
        </label>
      )}
    </section>
  );
}

export function QueuePanel({
  state,
  onPlayer,
  onClose,
  jam,
  jamInvite = false,
  onJamInvite,
  onJamJoin,
  onJamCopy,
  onJamEnd,
  onJamControl,
}: {
  state: AppState;
  onPlayer: (request: PlayerRequest) => void;
  onClose: () => void;
  jam?: Jam | null;
  jamInvite?: boolean;
  onJamInvite?: (open: boolean) => void;
  onJamJoin?: () => void;
  onJamCopy?: () => void;
  onJamEnd?: () => void;
  onJamControl?: (allowed: boolean) => void;
}) {
  const track = state.playback.item,
    raw = state.details.queue;
  // Spotify can report the advanced queue a moment before the song changes.
  // The leaving song stays listed until then, so the list moves once.
  const hold = useRef({
    track: "",
    queue: [] as Track[],
    queued: 0,
    since: 0,
  });
  const holdTimer = useRef(0);
  const [, wake] = useState(0);
  const held = hold.current;
  const early =
    !!track &&
    held.track === track.uri &&
    raw.length > 0 &&
    held.queue.length > 1 &&
    held.queue[0].uri !== raw[0].uri &&
    held.queue[1].uri === raw[0].uri &&
    (!held.since || performance.now() - held.since < 1500);
  const queue = early ? [held.queue[0], ...raw] : raw;
  // Songs the listener queued play first, then the playing context's.
  const queued = Math.min(
    queue.length,
    early ? held.queued : (state.details.queued ?? 0),
  );
  const dj = !!state.dj_context;
  const from =
    contextName(state.playback.context, {
      dj,
      playlists: state.playlists,
      track,
    }) ?? "";
  useEffect(() => {
    if (!early) {
      hold.current = {
        track: track?.uri ?? "",
        queue: raw,
        queued: state.details.queued ?? 0,
        since: 0,
      };
      return;
    }
    if (held.since) return;
    held.since = performance.now();
    clearTimeout(holdTimer.current);
    holdTimer.current = window.setTimeout(
      () => wake((count) => count + 1),
      1600,
    );
  });
  useEffect(() => () => clearTimeout(holdTimer.current), []);
  // Rows are keyed by song and repeat count, so the next song keeps its key
  // when it becomes Now playing and can slide up into that slot.
  const seen = new Map<string, number>();
  const keyOf = (item: Track) => {
    const count = seen.get(item.uri) ?? 0;
    seen.set(item.uri, count + 1);
    return `${item.uri}#${count}`;
  };
  const currentKey = track && !state.narrating ? keyOf(track) : "";
  const queueKeys = queue.map(keyOf);
  const rows = useRef(new Map<string, HTMLElement>());
  const tops = useRef(new Map<string, number>());
  const scroller = useRef<HTMLDivElement>(null);
  const order = [currentKey, ...queueKeys].join(" ");
  useLayoutEffect(() => {
    const box = scroller.current;
    if (!box) return;
    const origin = box.getBoundingClientRect().top - box.scrollTop;
    const previous = tops.current;
    const animate = previous.size > 0 && !state.settings.reduce_motion;
    const next = new Map<string, number>();
    const moves: [HTMLElement, number | undefined, number][] = [];
    for (const [key, element] of rows.current) {
      // A row still sliding is measured where it shows, then at rest, so a
      // new move continues from its place on screen instead of jumping.
      const shown = element.getBoundingClientRect().top - origin;
      for (const animation of element.getAnimations()) animation.cancel();
      const top = element.getBoundingClientRect().top - origin;
      next.set(key, top);
      const rest = previous.get(key);
      const before = rest === undefined ? undefined : rest + shown - top;
      if (animate && (before === undefined || Math.abs(before - top) > 0.5))
        moves.push([element, before, top]);
    }
    // Skip and Previous move the list by one place: rows slide that step
    // and songs joining the end fade in. Any other change (a reshuffle, a
    // new list) shows at once, since sliding or fading a whole list read as
    // the queue blanking and jumping.
    const moved = moves.filter(([, before]) => before !== undefined);
    const step = (element: HTMLElement, before: number, top: number) =>
      Math.abs(before - top) <= element.offsetHeight * 1.5;
    const stepped = moved.filter(([element, before, top]) =>
      step(element, before!, top),
    );
    const shifted = moved.length
      ? stepped.length >= moved.length * 0.8
      : moves.length <= 3;
    if (shifted)
      for (const [element, before, top] of moves) {
        if (before !== undefined && !step(element, before, top)) continue;
        element.animate(
          before !== undefined
            ? [
                { transform: `translateY(${before - top}px)` },
                { transform: "none" },
              ]
            : [{ opacity: 0 }, { opacity: 1 }],
          { duration: 260, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
        );
      }
    tops.current = next;
  }, [order, state.settings.reduce_motion]);
  const row = (item: Track, key: string, current = false, added = false) => (
    <button
      key={key}
      // Right-click opens the song menu; songs the listener queued can be
      // removed from it.
      data-context-route={item.id ? `track:${item.id}` : undefined}
      data-queued={added || undefined}
      ref={(element) => {
        if (element) rows.current.set(key, element);
        else rows.current.delete(key);
      }}
      className={`next-track queue-row ${current ? "current" : ""}`}
      onClick={
        current
          ? undefined
          : // Skip ahead to it, keeping the playlist and the rest of the queue.
            () => onPlayer({ command: "skip_to", uri: item.uri, track: item })
      }
      disabled={current}
    >
      <Cover images={item.album.images} />
      <div>
        <Marquee text={item.name} reduceMotion={state.settings.reduce_motion}>
          {item.name}
        </Marquee>
        <span>{artistNames(item)}</span>
      </div>
    </button>
  );
  return (
    <aside className="now-playing queue-panel">
      {/* Liquid Glass already draws this artwork behind the window. */}
      {!state.settings.liquid_glass && (
        <Ambience
          track={track}
          reduceMotion={state.settings.reduce_motion}
          animate={state.playback.is_playing}
        />
      )}
      <div className="now-playing-scroll" ref={scroller}>
        <div className="section-heading">
          <h2>Queue</h2>
          <div className="queue-header-actions">
            {!jam && onJamJoin && (
              <button
                className="text-link muted"
                onClick={onJamJoin}
                title="Join a Jam from a link you copied"
              >
                Join a Jam
              </button>
            )}
            <button
              className="icon-button"
              onClick={onClose}
              title="Close queue"
            >
              <X size={18} />
            </button>
          </div>
        </div>
        {jam && (
          <JamCard
            jam={jam}
            invite={jamInvite}
            hostName={state.profile?.display_name ?? ""}
            onInvite={(open) => onJamInvite?.(open)}
            onCopy={() => onJamCopy?.()}
            onEnd={() => onJamEnd?.()}
            onControl={(allowed) => onJamControl?.(allowed)}
          />
        )}
        {track && !state.narrating && (
          <section className="queue-section">
            <h3>Now playing</h3>
            {row(track, currentKey, true)}
          </section>
        )}
        {queued > 0 && (
          <section className="queue-section">
            <div className="queue-heading">
              <h3>Next in queue</h3>
              <button
                className="text-link muted"
                onClick={() => onPlayer({ command: "clear_queue" })}
              >
                Clear queue
              </button>
            </div>
            {queue
              .slice(0, queued)
              .map((item, index) => row(item, queueKeys[index], false, true))}
          </section>
        )}
        {/* DJ keeps its picks to itself, as in Spotify: only Now playing
            and songs the listener queued show. */}
        {!dj && (queued < queue.length || !queue.length) && (
          <section className="queue-section">
            <h3>{from ? `Next from: ${from}` : "Next up"}</h3>
            {queue.length ? (
              queue
                .slice(queued)
                .map((item, index) => row(item, queueKeys[queued + index]))
            ) : (
              <p className="queue-empty">Nothing is queued yet.</p>
            )}
          </section>
        )}
      </div>
    </aside>
  );
}

/** Performers plus songwriters, merged by name like Spotify's credits card. */
export function trackCredits(track: Track) {
  const credits = track.artists.map((artist) => ({
    name: artist.name,
    artist: artist as Artist | undefined,
    roles: ["Main Artist"],
  }));
  for (const writer of track.songwriters ?? []) {
    const existing = credits.find(
      (credit) => credit.name.toLowerCase() === writer.toLowerCase(),
    );
    if (existing) existing.roles.push("Writer");
    else credits.push({ name: writer, artist: undefined, roles: ["Writer"] });
  }
  return credits;
}

function CreditsCard({
  track,
  saved,
  onArtist,
  onFollow,
}: {
  track: Track;
  saved: Record<string, boolean>;
  onArtist: (artist: Artist) => void;
  onFollow: (artist: Artist) => void;
}) {
  const [all, setAll] = useState(false);
  useEffect(() => setAll(false), [track.uri]);
  const credits = trackCredits(track);
  // Show all opens Spotify's full Credits dialog, when Spotify knows the song.
  return (
    <article className="now-card">
      <div className="card-heading">
        <span>Credits</span>
        {(track.id || credits.length > 3) && (
          <button className="text-link muted" onClick={() => setAll(true)}>
            Show all
          </button>
        )}
      </div>
      {all && track.id && (
        <CreditsDialog
          track={track}
          saved={saved}
          onArtist={onArtist}
          onFollow={onFollow}
          onClose={() => setAll(false)}
        />
      )}
      <div className="card-content credit-list">
        {(all && !track.id ? credits : credits.slice(0, 3)).map((credit) => (
          <div key={credit.name} className="credit">
            <div>
              {credit.artist ? (
                <button
                  className="text-link"
                  onClick={() => onArtist(credit.artist!)}
                >
                  {credit.name}
                </button>
              ) : (
                <span className="credit-name">{credit.name}</span>
              )}
              <p>{credit.roles.join(" • ")}</p>
            </div>
            {credit.artist?.id && (
              <button
                className="pill outline"
                onClick={() => onFollow(credit.artist!)}
              >
                {saved[
                  credit.artist.uri ?? `spotify:artist:${credit.artist.id}`
                ]
                  ? "Following"
                  : "Follow"}
              </button>
            )}
          </div>
        ))}
      </div>
    </article>
  );
}
