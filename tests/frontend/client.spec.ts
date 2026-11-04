import { test, expect, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { emptyState } from "../../frontend/state";
import type { BrowseInfo, Track } from "../../frontend/types";

const appVersion: string = JSON.parse(
  readFileSync(new URL("../../package.json", import.meta.url), "utf8"),
).version;

const track: Track = {
  id: "0123456789abcdefghijkl",
  uri: "spotify:track:0123456789abcdefghijkl",
  name: "Timing test",
  artists: [{ id: "0123456789abcdefghijkl", name: "Test artist" }],
  album: {
    id: "0123456789abcdefghijkl",
    name: "Test album",
    images: [{ url: "https://artwork.example.test/cover.svg" }],
  },
  duration_ms: 180_000,
  is_local: false,
};
test("playing on another device does not silently redirect local Play", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: true,
          device: {
            id: "remote-device",
            name: "Other device",
            is_active: true,
          },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  await page.waitForTimeout(100);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => {
        const commands = (window as any).__clientTest.commands;
        return commands
          .filter((entry: any) => entry.command === "player")
          .at(-1)?.args.target;
      }),
    )
    .toBe("native");
});

test("artist links show the artist immediately while metadata is pending", async ({
  page,
}) => {
  await startClient(page);
  await page.locator(".player-bar .artist-link").first().click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Test artist",
  );
  await expect(page.locator(".skeleton-rows")).toHaveCount(0);
});

test("artist portraits replace missing banners and metadata stays below either header", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.route("https://artwork.example.test/artist/**", (route) =>
    route.fulfill({
      contentType: "image/svg+xml",
      headers: { "Access-Control-Allow-Origin": "*" },
      body: '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="640"><rect width="640" height="640" fill="#805982"/></svg>',
    }),
  );
  await page.locator(".player-bar .artist-link").first().click();
  const avatar = "https://artwork.example.test/artist/avatar.svg";
  const banner = "https://artwork.example.test/artist/banner.svg";
  const emitInfo = (info: BrowseInfo) =>
    page.evaluate((info) => {
      const host = (window as any).__clientTest;
      const generation = host.commands
        .filter(
          (entry: any) =>
            entry.command === "browse" && entry.args.key === info.key,
        )
        .at(-1).args.generation;
      host.emit({ type: "browse_info", generation, info });
    }, info);
  const info: BrowseInfo = {
    key: `artist:${track.artists[0].id}`,
    title: "Test artist",
    subtitle: "Artist",
    images: [{ url: avatar }],
    artist: {
      ...track.artists[0],
      images: [{ url: avatar }],
      verified: true,
      monthly_listeners: 1_400_000,
    },
    albums: [],
  };
  await emitInfo(info);
  const hero = page.locator(".artist-hero");
  await expect(hero).toHaveClass(/artist-fallback/);
  await expect(hero.locator(".hero-cover img")).toHaveAttribute("src", avatar);
  await expect(hero.locator(".artist-verified")).toHaveText(
    "Verified by Spotify",
  );
  await expect(hero.locator(".artist-monthly")).toHaveText(
    "1.4M monthly listeners",
  );
  const titleBox = (await hero.locator("h1").boundingBox())!;
  const verifiedBox = (await hero.locator(".artist-verified").boundingBox())!;
  expect(verifiedBox.y).toBeGreaterThanOrEqual(titleBox.y + titleBox.height);
  // The tick sits centred on its text, as in Spotify.
  const badge = (await hero.locator(".verified-badge").boundingBox())!;
  const label = (await hero.locator(".artist-verified span").boundingBox())!;
  expect(
    Math.abs(badge.y + badge.height / 2 - (label.y + label.height / 2)),
  ).toBeLessThan(1.5);
  // The badge itself is drawn, filled in Spotify's mint.
  expect(
    await hero
      .locator(".verified-badge > path")
      .evaluate((node) => getComputedStyle(node).fill),
  ).toBe("rgb(197, 240, 201)");
  // Artist pages reload when opened, so there is no Refresh.
  await expect(page.getByTitle("Refresh page")).toHaveCount(0);
  expect(
    await hero
      .locator(".hero-cover")
      .evaluate((node) => getComputedStyle(node).borderRadius),
  ).toBe("50%");
  await expect
    .poll(() =>
      hero.evaluate((node) =>
        getComputedStyle(node.parentElement!).getPropertyValue("--hero").trim(),
      ),
    )
    .not.toBe("rgb(83 83 83)");
  await hero.screenshot({ path: "target/diagnostics/artist-fallback.png" });

  const overview = {
    header_image: banner,
    verified: true,
    monthly_listeners: 72_800_000,
    biography: "",
    popular: [],
    source_url: "",
    fetched_at: 0,
  };
  await emitInfo({ ...info, overview });
  await expect(hero).not.toHaveClass(/artist-fallback/);
  await expect(hero.locator(".hero-cover")).toHaveCount(0);
  await expect(hero).toHaveCSS("background-image", /banner\.svg/);
  await expect(hero).toHaveCSS("background-size", "cover, cover");
  // Liquid Glass clears header backgrounds; the banner must still cover.
  await page.evaluate(() =>
    document.querySelector(".application")!.classList.add("liquid-glass"),
  );
  await expect(hero).toHaveCSS("background-size", "cover, cover");
  await page.evaluate(() =>
    document.querySelector(".application")!.classList.remove("liquid-glass"),
  );
  await expect(hero.locator(".artist-verified")).toHaveText(
    "Verified by Spotify",
  );
  await expect(hero.locator(".artist-monthly")).toHaveText(
    "72.8M monthly listeners",
  );
  const bannerTitleBox = (await hero.locator("h1").boundingBox())!;
  const bannerVerifiedBox = (await hero
    .locator(".artist-verified")
    .boundingBox())!;
  expect(bannerVerifiedBox.y).toBeGreaterThanOrEqual(
    bannerTitleBox.y + bannerTitleBox.height,
  );
  await hero.screenshot({ path: "target/diagnostics/artist-banner.png" });

  await emitInfo({
    ...info,
    overview: {
      ...overview,
      header_image: undefined,
      verified: false,
      monthly_listeners: 0,
    },
  });
  await expect(hero).toHaveClass(/artist-fallback/);
  await expect(hero.locator(".hero-cover")).toBeVisible();
  await expect(hero.locator(".artist-verified")).toHaveCount(0);
  await expect(hero.locator(".artist-monthly")).toHaveText(
    "0 monthly listeners",
  );
  await page.setViewportSize({ width: 1024, height: 900 });
  expect(
    await hero.evaluate((node) => node.scrollWidth <= node.clientWidth),
  ).toBe(true);
});

test("Connect retains registered speakers and transfers back to this computer", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "devices",
      devices: [
        { id: "registered-speaker", name: "Test Echo", is_active: false },
      ],
    }),
  );
  await page.getByTitle("Devices", { exact: true }).click();
  await page.getByRole("button", { name: /Test Echo/ }).click();
  await page.getByTitle("Devices", { exact: true }).click();
  await page.getByRole("button", { name: /^This computer/ }).click();
  const transfers = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter(
        (entry: any) =>
          entry.command === "player" &&
          entry.args.request.command === "transfer",
      )
      .map((entry: any) => entry.args),
  );
  expect(transfers).toEqual([
    {
      request: { command: "transfer", uri: "registered-speaker" },
      target: "registered-speaker",
    },
    { request: { command: "transfer", uri: "native" }, target: "native" },
  ]);
});

test("standby speakers are labelled and a failed transfer keeps controls on the active device", async ({
  page,
}) => {
  await startClient(page);
  const devices = [
    { id: "phone", name: "Test Phone", is_active: true },
    {
      id: "standby-echo",
      name: "Test Echo",
      is_active: false,
      is_offline: true,
    },
  ];
  await page.evaluate(
    (devices) =>
      (window as any).__clientTest.emit({ type: "devices", devices }),
    devices,
  );
  await page.getByTitle("Devices", { exact: true }).click();
  const echo = page.getByRole("button", { name: /Test Echo/ });
  await expect(echo).toContainText("Standby");
  await expect(page.getByRole("button", { name: /Test Phone/ })).toContainText(
    "Active device",
  );
  await echo.click();
  // The backend re-sends the device list after Spotify rejects the transfer.
  await page.evaluate(
    (devices) =>
      (window as any).__clientTest.emit({ type: "devices", devices }),
    devices,
  );
  await page
    .locator("footer")
    .getByRole("button", { name: "Play", exact: true })
    .click();
  const sent = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "player")
      .map((entry: any) => entry.args.target),
  );
  expect(sent).toEqual(["standby-echo", "phone"]);
});

test("audio output selection saves headphones and can return to system default", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page);
  const output = page.getByRole("combobox", {
    name: "Audio output",
    exact: true,
  });
  await expect(output.locator("option")).toHaveCount(3);
  await output.selectOption("Test headphones");
  await expect(output).toHaveValue("Test headphones");
  const savedOutput = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings.audio_output,
    );
  await expect.poll(savedOutput).toBe("Test headphones");
  await output.selectOption("");
  await expect.poll(savedOutput).toBe(null);
  const outputRequests = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "audio_outputs",
        ).length,
    );
  const previousRequests = await outputRequests();
  await page
    .getByRole("button", { name: "Refresh outputs", exact: true })
    .click();
  await expect.poll(outputRequests).toBe(previousRequests + 1);
  await page.getByRole("button", { name: "Test output", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (entry: any) => entry.command === "test_audio_output",
          ).length,
      ),
    )
    .toBe(1);
});

test("bundled lyrics stay ready after sign-out and another sign-in", async ({
  page,
}) => {
  await startClient(page, "liked", true);
  await openSettings(page, "Sources");
  await expect(
    page.getByRole("button", { name: "Save lyrics key", exact: true }),
  ).toHaveCount(0);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText(
    "Spotify Premium required",
  );
  await page.keyboard.press("Escape");
  await expect(page.locator(".setup-backdrop")).toBeVisible();
  await page.evaluate(() => {
    (window as any).__clientTest.emit({
      type: "session",
      profile: { id: "another-listener", display_name: "Listener", images: [] },
      can_follow: true,
    });
  });
  await expect(page.locator(".application")).toBeVisible();
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toHaveCount(0);
  await openSettings(page, "Sources");
  await expect(
    page.getByRole("button", { name: "Save lyrics key", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("Lyrics API key", { exact: true })).toHaveCount(
    0,
  );
});

test("first-run Spotify sign-in needs no developer Client ID", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Spotify Premium required");
  await expect(dialog.locator("#client-id")).toHaveCount(0);
  // Signed out, the top bar keeps only the app menu and window controls.
  await expect(page.getByPlaceholder("What do you want to play?")).toBeHidden();
  await expect(page.getByTitle("Go back", { exact: true })).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Account menu", exact: true }),
  ).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Minimize window", exact: true }),
  ).toBeVisible();
  // The ambience is drawn by the dithered WebGL shader, not CSS gradients.
  await expect(page.locator(".setup-ambience canvas")).toHaveAttribute(
    "data-ready",
    "true",
  );
  const signIn = dialog.getByRole("button", {
    name: "Sign in with Spotify",
    exact: true,
  });
  await expect(signIn).toBeEnabled();
  // Hovering must not grow the button past the column or scroll it.
  const before = await signIn.boundingBox();
  await signIn.hover();
  await page.waitForTimeout(300);
  expect(await signIn.boundingBox()).toEqual(before);
  expect(
    await dialog.evaluate(
      (node) =>
        node.scrollWidth <= node.clientWidth &&
        node.scrollHeight <= node.clientHeight,
    ),
  ).toBe(true);
  await signIn.click();
  const request = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "sign_in")
      .at(-1),
  );
  expect(request.args.clientId).toBe("");
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: /close|skip/i })).toHaveCount(
    0,
  );
});

test("developer connection remains optional and validates its Client ID", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("button", {
      name: "Developer connection (optional)",
      exact: true,
    })
    .click();
  const signIn = dialog.getByRole("button", {
    name: "Sign in with Spotify",
    exact: true,
  });
  await dialog.locator("#client-id").fill("invalid");
  await expect(signIn).toBeDisabled();
  await dialog.locator("#client-id").fill("a".repeat(32));
  await signIn.click();
  const request = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "sign_in")
      .at(-1),
  );
  expect(request.args.clientId).toBe("a".repeat(32));
});

test("failed saved sign-in stays locked, supports retry and cancellable fresh sign-in", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ settings }) => {
      const emit = (window as any).__clientTest.emit;
      emit({ type: "signed_out" });
      emit({
        type: "settings",
        settings: { ...settings, client_id: "0".repeat(32) },
      });
      emit({
        type: "login_failed",
        issue: {
          message: "Cannot reach Spotify sign-in service",
          retry_saved: true,
        },
      });
    },
    { settings: emptyState.settings },
  );
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText("Cannot reach Spotify");
  await expect(dialog).toContainText("Spotify Premium required");
  await page.keyboard.press("Escape");
  await page.locator(".setup-backdrop").click({ position: { x: 2, y: 2 } });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: /close|skip/i })).toHaveCount(
    0,
  );
  await dialog
    .getByRole("button", { name: "Retry saved sign-in", exact: true })
    .click();
  await expect(
    page.getByRole("status", { name: "Restoring session" }),
  ).toBeVisible();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "login_failed",
      issue: {
        message: "Spotify rejected the saved sign-in. Sign in again.",
        retry_saved: false,
      },
    }),
  );
  await expect(
    dialog.getByRole("button", { name: "Retry saved sign-in", exact: true }),
  ).toHaveCount(0);
  await dialog
    .getByRole("button", { name: "Sign in again", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Connecting…", exact: true }),
  ).toBeDisabled();
  await dialog
    .getByRole("button", { name: "Cancel sign-in", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Sign in with Spotify", exact: true }),
  ).toBeEnabled();
  await expect(dialog).toBeVisible();
  const calls = await page.evaluate(() =>
    (window as any).__clientTest.commands.map((entry: any) => entry.command),
  );
  expect(calls).toContain("retry_sign_in");
  expect(calls).toContain("sign_in");
});

test("metadata menus stay off blank rows, backgrounds and player controls", async ({
  page,
}) => {
  await startClient(page);
  await page
    .locator(".track-row .track-name")
    .first()
    .click({ button: "right" });
  await expect(page.getByRole("menu")).toBeVisible();
  const row = await page.locator(".track-row").first().boundingBox();
  if (!row) throw Error("Track row missing");
  await page.mouse.click(row.x + 12, row.y + row.height / 2, {
    button: "right",
  });
  await expect(page.getByRole("menu")).toHaveCount(0);
  await page
    .locator(".page-hero")
    .click({ button: "right", position: { x: 5, y: 5 } });
  await expect(page.getByRole("menu")).toHaveCount(0);
  await page
    .locator('.player-song button[title="Save to Liked Songs"]')
    .click({ button: "right" });
  await expect(page.getByRole("menu")).toHaveCount(0);
  await page.locator(".album-link").first().click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Go to album", exact: true }),
  ).toBeVisible();
});

async function showLyrics(page: Page) {
  const settings = page.getByRole("dialog", { name: "Lyssna settings" });
  if (await settings.count())
    await settings.getByRole("button", { name: "Close settings" }).click();
  if (!(await page.locator(".lyrics-page").count()))
    await page.getByTitle("Lyrics", { exact: true }).click();
}
/** Opens the Lyssna settings window, unless it is open, on a tab. */
async function openSettings(page: Page, tab = "General") {
  const dialog = page.getByRole("dialog", { name: "Lyssna settings" });
  if (!(await dialog.count())) {
    await page
      .getByRole("button", { name: "Account menu", exact: true })
      .click();
    await page.getByRole("menuitem", { name: "Settings", exact: true }).click();
  }
  await page.getByRole("tab", { name: tab, exact: true }).click();
}
async function startClient(
  page: Page,
  initialRoute = "liked",
  bundledLyrics = false,
) {
  await page.route("https://artwork.example.test/**", (route) =>
    route.fulfill({
      contentType: "image/svg+xml",
      body: '<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><rect width="300" height="300" fill="#9670a7"/></svg>',
    }),
  );
  await page.addInitScript(
    ({ snapshot, track }) => {
      const host = window as any;
      const callbacks = new Map<number, Function>();
      const handlers = new Map<number, { event: string; handler: number }>();
      const commands: { command: string; args: any }[] = [];
      let serial = 0;
      const emitNamed = (event: string, payload: any) =>
        handlers.forEach(
          (handler) =>
            handler.event === event &&
            callbacks.get(handler.handler)?.({ event, payload }),
        );
      const emit = (payload: any) => emitNamed("lyssna", payload);
      host.__clientTest = {
        commands,
        emit,
        emitVoice: (payload: any) => emitNamed("lyssna-dj-voice", payload),
        emitNamed,
        pendingVoice: null,
        resolveVoice: null,
      };
      host.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: (_: string, id: number) => handlers.delete(id),
      };
      host.__TAURI_INTERNALS__ = {
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
        transformCallback: (callback: Function) => {
          callbacks.set(++serial, callback);
          return serial;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        invoke: async (command: string, args: any = {}) => {
          commands.push({ command, args });
          if (command === "audio_outputs")
            return ["Test speakers", "Test headphones"];
          if (command === "artist_liked_count")
            return host.__clientTest.artistLiked ?? 0;
          if (command === "track_credits")
            return {
              name: "Timing test",
              groups: [
                {
                  name: "Artist",
                  credits: [
                    {
                      name: "Test artist",
                      roles: ["Main Artist"],
                      uri: "spotify:artist:0123456789abcdefghijkl",
                    },
                  ],
                },
                {
                  name: "Composition & Lyrics",
                  credits: [
                    { name: "Writer One", roles: ["Lyricist"], uri: null },
                  ],
                },
                {
                  name: "Production & Engineering",
                  credits: [
                    { name: "Maker Two", roles: ["Producer"], uri: null },
                  ],
                },
              ],
              sources: ["Test Label"],
            };
          if (command === "jam") {
            const test = host.__clientTest;
            if (args.action === "start") test.jam = test.jamToStart;
            if (args.action === "join")
              test.jam = {
                ...test.jamToStart,
                is_owner: false,
                owner_id: "friend",
                members: [
                  {
                    id: "friend",
                    username: "friend",
                    display_name: "Friend",
                    image_url: null,
                  },
                ],
              };
            if (args.action === "end" || args.action === "leave")
              test.jam = null;
            if (args.action === "control" && test.jam)
              test.jam = { ...test.jam, queue_only: !args.allowed };
            return test.jam ?? null;
          }
          if (command === "artist_discography")
            return (window as any).__discography?.pending
              ? new Promise(() => {})
              : ((window as any).__discography?.list ?? []);
          if (command === "release_tracks")
            return (window as any).__discography?.tracks?.[args.id] ?? [[], {}];
          if (command === "liked_songs_count")
            return host.__clientTest.likedTotal ?? 0;
          if (command === "read_clipboard") {
            if (!host.__clientTest.clipboard)
              throw new Error("Clipboard has no preset code.");
            return host.__clientTest.clipboard;
          }
          if (command === "list_duration") {
            if (!host.__clientTest.listDuration)
              throw new Error("Length unavailable");
            return host.__clientTest.listDuration;
          }
          if (command === "saved_lyrics")
            return host.__clientTest.savedLyrics ?? [];
          if (command === "saved_lyrics_file")
            return ["Artist - Saved song.ttml", "<tt/>"];
          if (command === "audio_cache_size")
            return host.__clientTest.cacheBytes ?? 0;
          if (command === "clear_audio_cache") {
            const freed = host.__clientTest.cacheBytes ?? 0;
            host.__clientTest.cacheBytes = 0;
            return freed;
          }
          if (command === "test_audio_output") return true;
          if (command === "nightly_status")
            return {
              commit: (window as any).__nightlyCommit ?? null,
              report: null,
            };
          if (command === "nightly_check")
            return host.__clientTest.nightlyCheck ?? "current";
          if (command === "nightly_build") return true;
          if (command === "nightly_clear_cache") return 3 * 1_048_576 * 1024;
          if (command === "take_link") {
            const link = host.__clientTest.link ?? null;
            host.__clientTest.link = null;
            return link;
          }
          if (command === "musixmatch_token_saved")
            return !!host.__clientTest.musixmatchToken;
          if (command === "save_musixmatch_token") {
            host.__clientTest.musixmatchToken = args?.token ?? "";
            return;
          }
          if (command === "renew_musixmatch_token") return true;
          if (command === "cancel_sign_in") {
            emit({ type: "login_finished" });
            return;
          }
          if (command === "dj_voice") {
            host.__clientTest.emitVoice({
              id: args.id,
              account: args.account,
              level: 0,
              speech: false,
              stage: "listening",
            });
            return new Promise<string>((resolve) => {
              host.__clientTest.resolveVoice = resolve;
            });
          }
          if (command === "dj_request" && host.__clientTest.holdDJRequest)
            return new Promise<void>((resolve) => {
              host.__clientTest.resolveDJRequest = resolve;
            });
          if (command === "follow_user") return args.follow;
          if (command === "whats_new") return host.__clientTest.whatsNew ?? [];
          if (command === "library_items")
            return (
              host.__clientTest.libraryItems ?? { albums: [], artists: [] }
            );
          if (command === "search_page")
            return {
              more: false,
              results: {
                tracks:
                  args.kind === "tracks"
                    ? Array.from({ length: 5 }, (_, index) => {
                        const id = String(index + 21).padStart(22, "0");
                        return {
                          ...track,
                          id,
                          uri: `spotify:track:${id}`,
                          name: `Song ${index + 21}`,
                        };
                      })
                    : [],
                artists: [],
                albums: [],
                playlists: [],
                users: [],
                top_results: [],
              },
            };
          if (command === "user_profile") {
            const profile = host.__clientTest.userProfiles?.[args.user];
            return profile
              ? structuredClone(profile)
              : Promise.reject("Profile unavailable");
          }
          if (command === "resolve_album")
            return {
              ...track.album,
              id: "abcdefghijklmnopqrstuv",
              uri: "spotify:album:abcdefghijklmnopqrstuv",
            };
          if (command === "plugin:event|listen") {
            handlers.set(++serial, {
              event: args.event,
              handler: args.handler,
            });
            return serial;
          }
          if (command === "plugin:event|unlisten") {
            handlers.delete(args.eventId);
            return;
          }
          if (command === "bootstrap") return structuredClone(snapshot);
          if (command === "home") {
            emit({ type: "home_loading", account: "test-user" });
            setTimeout(
              () =>
                emit({
                  type: "home",
                  page: {
                    account: "test-user",
                    greeting: "Hello",
                    sections: [
                      {
                        uri: "daily-mixes",
                        title: "Daily Mixes",
                        subtitle: "",
                        items: [
                          {
                            uri: "spotify:playlist:1123456789abcdefghijkl",
                            route: "playlist:1123456789abcdefghijkl",
                            name: "Daily Mix 1",
                            subtitle: "Your favorite artists",
                            images: track.album.images,
                            kind: "playlist",
                            playable: true,
                            format: "daily-mix",
                          },
                        ],
                      },
                      {
                        uri: "personal-releases",
                        title: "New releases for you",
                        subtitle: "New music from artists you love",
                        items: [
                          {
                            uri: "spotify:album:2123456789abcdefghijkl",
                            route: "album:2123456789abcdefghijkl",
                            name: "Your new album",
                            subtitle: "Your artist",
                            images: track.album.images,
                            kind: "album",
                            playable: true,
                            format: "",
                          },
                        ],
                      },
                      {
                        uri: "personal-stations",
                        title: "Recommended Stations",
                        subtitle: "",
                        items: [
                          {
                            uri: "spotify:playlist:3123456789abcdefghijkl",
                            route: "playlist:3123456789abcdefghijkl",
                            name: "Your station",
                            subtitle: "Based on your music",
                            images: track.album.images,
                            kind: "playlist",
                            playable: true,
                            format: "radio",
                          },
                        ],
                      },
                    ],
                  },
                }),
              20,
            );
            return;
          }
          if (command === "browse") {
            // Recents tests supply their own dated play history.
            if (args.key === "home" || args.key === "recent") return;
            if (args.key.startsWith("search:")) {
              setTimeout(
                () =>
                  emit({
                    type: "search_results",
                    account: "test-user",
                    key: args.key,
                    generation: args.generation,
                    message: "",
                    results: {
                      tracks: Array.from({ length: 20 }, (_, index) => {
                        const id = String(index + 1).padStart(22, "0");
                        return {
                          ...track,
                          id,
                          uri: `spotify:track:${id}`,
                          name: `Song ${index + 1}`,
                        };
                      }),
                      artists: [
                        {
                          ...track.artists[0],
                          uri: `spotify:artist:${track.artists[0].id}`,
                          images: track.album.images,
                        },
                      ],
                      albums: [
                        {
                          ...track.album,
                          uri: `spotify:album:${track.album.id}`,
                        },
                      ],
                      playlists: [],
                      users: [
                        {
                          id: "friend",
                          display_name: "Friend Person",
                          images: track.album.images,
                        },
                      ],
                      top_results: [
                        "spotify:track:0000000000000000000001",
                        `spotify:artist:${track.artists[0].id}`,
                      ],
                    },
                  }),
                20,
              );
              return;
            }
            const offset = args.offset ?? 0;
            setTimeout(
              () =>
                emit({
                  type: "tracks",
                  key: args.key,
                  generation: args.generation,
                  offset,
                  more: offset < 100,
                  tracks: Array.from({ length: 50 }, (_, index) => ({
                    ...track,
                    uri: `spotify:track:item${offset + index}`,
                    name: `Song ${offset + index + 1}`,
                  })),
                }),
              offset ? 300 : 20,
            );
          }
          if (command === "import_lyrics") {
            emit({
              type: "lyrics",
              uri: track.uri,
              lyrics: {
                source: "Imported TTML",
                synced: true,
                instrumental: false,
                songwriters: ["Test Writer"],
                lines: Array.from({ length: 80 }, (_, index) => ({
                  text: `Test lyric line ${index + 1}`,
                  start_ms: index * 2000,
                  end_ms: (index + 1) * 2000,
                  translation: "",
                  romanization: "",
                  words: [
                    {
                      text: `Test lyric line ${index + 1}`,
                      start_ms: index * 2000,
                      end_ms: (index + 1) * 2000,
                    },
                  ],
                  background: false,
                  opposite: false,
                })),
              },
            });
          }
        },
      };
    },
    {
      snapshot: {
        ...emptyState,
        provider_builtin: bundledLyrics,
        provider_configured: bundledLyrics,
        provider_kind: bundledLyrics ? "spicy" : undefined,
        route: initialRoute,
        profile: { id: "test-user" },
        local_device_name: "Lyssna (test)",
        native_ready: true,
        playback: {
          ...emptyState.playback,
          item: track,
          progress_ms: 40_000,
          device: {
            name: "Lyssna (test)",
            id: "local-device",
            is_active: true,
          },
        },
        details: { artists: track.artists, queue: [] },
      },
      track,
    },
  );
  await page.goto("/");
  await page
    .locator(".application")
    .waitFor({ state: "visible", timeout: 20_000 });
  await expect(page.locator(".setup-backdrop")).toHaveCount(0);
  if (initialRoute === "home")
    await expect(
      page.getByRole("heading", { name: "Daily Mixes", exact: true }),
    ).toBeVisible();
  else if (!initialRoute.startsWith("discography:"))
    await expect(page.locator(".track-row")).toHaveCount(50);
}

/** The player's DJ button only shows while a DJ session is playing. */
async function inDJSession(page: Page) {
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "dj_context", active: true }),
  );
}

async function askDJ(page: Page) {
  await inDJSession(page);
  await page.getByRole("button", { name: "Refresh DJ", exact: true }).focus();
  await page.keyboard.press("Shift+Enter");
  await expect(page.getByRole("dialog", { name: "Ask DJ" })).toBeVisible();
}

test("DJ sends arbitrary typed requests without opening the microphone", async ({
  page,
}) => {
  await startClient(page);
  await askDJ(page);
  await page
    .getByRole("textbox", { name: "DJ request", exact: true })
    .fill("Energetic 2000s rock, then calm ambient music");
  await page.getByRole("button", { name: "Send to DJ", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Ask DJ" })).toHaveCount(0);
  const commands = await page.evaluate(
    () => (window as any).__clientTest.commands,
  );
  expect(commands.filter((c: any) => c.command === "dj_voice")).toHaveLength(0);
  expect(
    commands.find((c: any) => c.command === "dj_request").args,
  ).toMatchObject({
    account: "test-user",
    input: "text",
    text: "Energetic 2000s rock, then calm ambient music",
  });
  expect(commands.filter((c: any) => c.command === "dj_cancel")).toHaveLength(
    0,
  );
});

test("DJ voice submits after speech ends without filling text and rejects cancelled results", async ({
  page,
}) => {
  await startClient(page);
  await askDJ(page);
  await page.getByRole("button", { name: "Use microphone" }).click();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() =>
    (window as any).__clientTest.resolveVoice("Do not apply this old result"),
  );
  await expect(
    page.getByRole("textbox", { name: "DJ request", exact: true }),
  ).toHaveValue("");
  await page
    .getByRole("textbox", { name: "DJ request", exact: true })
    .fill("Keep my typed draft");
  await page.evaluate(
    () => ((window as any).__clientTest.holdDJRequest = true),
  );
  await page.getByRole("button", { name: "Use microphone" }).click();
  await page.evaluate(() =>
    (window as any).__clientTest.resolveVoice("Play acoustic folk"),
  );
  await expect(
    page.getByRole("textbox", { name: "DJ request", exact: true }),
  ).toHaveValue("Keep my typed draft");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.find(
            (c: any) => c.command === "dj_request",
          )?.args,
      ),
    )
    .toMatchObject({ text: "Play acoustic folk", input: "voice" });
  await expect(
    page.getByText("Heard: “Play acoustic folk”", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (c: any) => c.command === "dj_request",
        ).length,
    ),
  ).toBe(1);
  await page.evaluate(() => (window as any).__clientTest.resolveDJRequest());
  await expect(page.getByRole("dialog", { name: "Ask DJ" })).toHaveCount(0);
  await expect(
    page.getByText("DJ heard: “Play acoustic folk”", { exact: true }),
  ).toBeVisible();
});

test("DJ microphone bars follow real level events and ignore other captures", async ({
  page,
}) => {
  await startClient(page);
  await askDJ(page);
  await page.getByRole("button", { name: "Use microphone" }).click();
  const meter = page.getByRole("meter", { name: "Microphone input" });
  await expect(meter).toHaveAttribute("aria-valuenow", "0");
  await expect(meter.locator("i")).toHaveCount(11);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const id = host.commands.find((c: any) => c.command === "dj_voice").args.id;
    host.voiceId = id;
    host.emitVoice({ id, account: "test-user", level: 75, speech: false });
  });
  await expect(meter).toHaveAttribute("aria-valuenow", "75");
  await expect(page.getByText("Sound detected", { exact: true })).toBeVisible();
  await expect
    .poll(() =>
      meter
        .locator("i")
        .first()
        .evaluate((e) => e.getBoundingClientRect().height),
    )
    .toBeGreaterThan(3);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emitVoice({
      id: "old-capture",
      account: "test-user",
      level: 99,
      speech: true,
    });
    host.emitVoice({
      id: host.voiceId,
      account: "other-user",
      level: 99,
      speech: true,
    });
  });
  await expect(meter).toHaveAttribute("aria-valuenow", "75");
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emitVoice({
      id: host.voiceId,
      account: "test-user",
      level: 0,
      speech: true,
    });
  });
  await expect(meter).toHaveAttribute("aria-valuenow", "0");
  await expect(page.getByText("Hearing speech", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(meter).toHaveCount(0);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emitVoice({
      id: host.voiceId,
      account: "test-user",
      level: 100,
      speech: true,
    });
    host.resolveVoice("Old request");
  });
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) => c.command === "dj_request",
      ),
    ),
  ).toBe(false);
  await expect(meter).toHaveCount(0);
});

test("Ask DJ prewarms requests without starting capture and releases its model on close", async ({
  page,
}) => {
  await startClient(page);
  await askDJ(page);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) =>
          c.command === "dj_prepare" && c.args.account === "test-user",
      ),
    ),
  ).toBe(true);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) => c.command === "dj_voice",
      ),
    ),
  ).toBe(false);
  await page.getByRole("button", { name: "Use microphone" }).click();
  const dialog = await page.evaluate(
    () =>
      (window as any).__clientTest.commands.find(
        (c: any) => c.command === "dj_voice",
      ).args.dialog,
  );
  await page.getByRole("button", { name: "Close DJ request" }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands
          .filter((c: any) => c.command === "dj_release_voice")
          .map((c: any) => c.args.dialog),
      ),
    )
    .toContain(dialog);
  await page.evaluate(() =>
    (window as any).__clientTest.resolveVoice("Do not submit after close"),
  );
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) => c.command === "dj_request",
      ),
    ),
  ).toBe(false);
});

test("DJ waits for local speech setup before inviting the user to speak", async ({
  page,
}) => {
  await startClient(page);
  await askDJ(page);
  await page.getByRole("button", { name: "Use microphone" }).click();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const id = host.commands.find((c: any) => c.command === "dj_voice").args.id;
    host.voiceId = id;
    host.emitVoice({
      id,
      account: "test-user",
      level: 0,
      speech: false,
      stage: "preparing",
      percent: 37,
    });
  });
  await expect(page.getByRole("status")).toHaveText(
    "Preparing local speech recognition… 37%",
  );
  await expect(
    page.getByRole("meter", { name: "Microphone input" }),
  ).toHaveCount(0);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emitVoice({
      id: host.voiceId,
      account: "test-user",
      level: 30,
      speech: true,
      stage: "listening",
    });
  });
  await expect(
    page.getByRole("meter", { name: "Microphone input" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emitVoice({
      id: host.voiceId,
      account: "test-user",
      level: 0,
      speech: false,
      stage: "transcribing",
    });
  });
  await expect(page.getByRole("status")).toHaveText(
    "Recognizing your request…",
  );
  await expect(
    page.getByRole("meter", { name: "Microphone input" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.evaluate(() =>
    (window as any).__clientTest.resolveVoice("Old voice request"),
  );
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) => c.command === "dj_request",
      ),
    ),
  ).toBe(false);
});

test("DJ dialog traps focus and closes on sign-out", async ({ page }) => {
  await startClient(page);
  await askDJ(page);
  await page.getByRole("button", { name: "Close DJ request" }).focus();
  await page.keyboard.press("Shift+Tab");
  await expect(
    page.getByRole("button", { name: "Use microphone" }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "Close DJ request" }),
  ).toBeFocused();
  await page.getByRole("button", { name: "Use microphone" }).click();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  await expect(page.getByRole("dialog", { name: "Ask DJ" })).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (c: any) => c.command === "dj_cancel",
      ),
    ),
  ).toBe(true);
});

// Plays a short recorded purple video as the current track's Canvas.
async function emitCanvas(page: Page) {
  await page.evaluate(async (uri) => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 64;
    const context = canvas.getContext("2d")!;
    context.fillStyle = "#7547c2";
    context.fillRect(0, 0, 64, 64);
    const recorder = new MediaRecorder(canvas.captureStream(2));
    const parts: BlobPart[] = [];
    const url = await new Promise<string>((resolve) => {
      recorder.ondataavailable = (event) => parts.push(event.data);
      recorder.onstop = () =>
        resolve(
          URL.createObjectURL(new Blob(parts, { type: recorder.mimeType })),
        );
      recorder.start();
      setTimeout(() => recorder.stop(), 300);
    });
    (window as any).__clientTest.emit({ type: "canvas", uri, url });
    (window as any).__clientTest.emit({
      type: "provider_configured",
      kind: "spicy",
    });
  }, track.uri);
}

test("sidebar preserves pins and changes recents only after manual playback", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    (window as any).__clientTest.emit({
      type: "library",
      playlists: Array.from({ length: 5 }, (_, index) => ({
        id: String(index).padStart(22, "0"),
        name: `Playlist ${index}`,
        images: [],
        owner: { id: "test-user" },
        collaborative: false,
      })),
    });
  });
  const names = () => page.locator(".library-row strong").allTextContents();
  await expect(page.locator(".library-row strong").first()).toHaveText("DJ");
  await page
    .locator(".library-row")
    .filter({ has: page.getByText("Playlist 2", { exact: true }) })
    .locator(".library-item")
    .click();
  expect((await names())[0]).toBe("DJ");
  await page
    .getByRole("main")
    .getByTitle("Play Playlist 2", { exact: true })
    .click();
  await expect(page.locator(".library-row strong").first()).toHaveText(
    "Playlist 2",
  );
  for (const name of ["Playlist 0", "Playlist 1", "Playlist 3"]) {
    await page
      .locator(".library-row")
      .filter({ has: page.getByText(name, { exact: true }) })
      .click({ button: "right" });
    await page
      .getByRole("menuitem", { name: "Pin playlist", exact: true })
      .click();
  }
  expect((await names()).slice(0, 3)).toEqual([
    "Playlist 0",
    "Playlist 1",
    "Playlist 3",
  ]);
  await page
    .locator(".library-row")
    .filter({ has: page.getByText("Playlist 4", { exact: true }) })
    .click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Pin playlist", exact: true })
    .click();
  await expect(
    page.getByText("Four pins maximum, including Liked Songs.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    page.locator(".library-list > .library-liked > .library-item"),
  ).toContainText("Liked Songs");
  await expect(
    page.locator(".library-list > .library-liked .pinned-mark"),
  ).toHaveCount(0);
});
test("artist and song right clicks offer relevant actions", async ({
  page,
}) => {
  await startClient(page);
  await page
    .locator(".player-song .artist-link")
    .first()
    .click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Go to artist", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Follow artist", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Pin playlist", exact: true }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
  await page
    .locator(".track-row .track-name")
    .first()
    .click({ button: "right" });
  await expect(
    page.getByRole("menuitem", { name: "Add to queue", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Save to Liked Songs", exact: true }),
  ).toBeVisible();
});
test("DJ speech replaces song metadata across player, Now playing and lyrics", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "narration", active: true }),
  );
  await expect(page.locator(".player-song")).toContainText("DJ");
  await expect(page.locator(".now-playing")).toContainText("DJ");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toContainText("DJ");
  await expect(page.locator(".lyrics-page")).not.toContainText("Timing test");
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "narration", active: false }),
  );
  await expect(page.locator(".player-song")).toContainText("Timing test");
  await expect(page.locator(".lyrics-page")).toContainText("Timing test");
});

test("holding hint appears at DJ startup and stays quiet during later segments", async ({
  page,
}) => {
  await startClient(page);
  await expect(
    page.getByText("Hold the DJ button to ask for music.", { exact: true }),
  ).toHaveCount(0);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "narration", active: true }),
  );
  await expect(
    page.getByText("Hold the DJ button to ask for music.", { exact: true }),
  ).toBeVisible();
  const first = await page.evaluate(() =>
    localStorage.getItem("lyssna.dj-hint.test-user"),
  );
  expect(Number(first)).toBeGreaterThan(0);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emit({ type: "notice", message: "Other notice", error: false });
    host.emit({ type: "narration", active: false });
  });
  await expect(page.getByText("Other notice", { exact: true })).toBeVisible();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "narration", active: true }),
  );
  await expect(page.getByText("Other notice", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(() => localStorage.getItem("lyssna.dj-hint.test-user")),
  ).toBe(first);
});
test("saved preferences from before the rename carry over to Lyssna", async ({
  page,
}) => {
  await page.addInitScript(() => {
    localStorage.setItem("spotiflex.dj-hint.test-user", "1234");
    localStorage.setItem("spotiflex.settings-place", "old");
    localStorage.setItem("lyssna.settings-place", "new");
  });
  await startClient(page);
  expect(
    await page.evaluate(() => ({
      hint: localStorage.getItem("lyssna.dj-hint.test-user"),
      place: localStorage.getItem("lyssna.settings-place"),
      old: Object.keys(localStorage).filter((key) =>
        key.startsWith("spotiflex."),
      ),
    })),
  ).toEqual({ hint: "1234", place: "new", old: [] });
});
test("player DJ button requests a fresh DJ selection on every click", async ({
  page,
}) => {
  await startClient(page);
  const button = page.getByRole("button", { name: "Refresh DJ", exact: true });
  await expect(button).toHaveCount(0);
  await inDJSession(page);
  await button.click();
  await button.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (entry: any) =>
              entry.command === "player" && entry.args.request.command === "dj",
          ).length,
      ),
    )
    .toBe(2);
});

test("holding DJ opens requests without refreshing", async ({ page }) => {
  await startClient(page);
  await page.clock.install();
  await inDJSession(page);
  const button = page.getByRole("button", { name: "Refresh DJ", exact: true });
  await button.hover();
  await page.mouse.down();
  await page.clock.runFor(600);
  await expect(page.getByRole("dialog", { name: "Ask DJ" })).toBeVisible({
    timeout: 1000,
  });
  await page.mouse.up();
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) =>
            entry.command === "player" && entry.args.request.command === "dj",
        ).length,
    ),
  ).toBe(0);
  await page.getByRole("button", { name: "Close DJ request" }).click();
  await button.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (entry: any) =>
              entry.command === "player" && entry.args.request.command === "dj",
          ).length,
      ),
    )
    .toBe(1);
});

test("cancelled DJ holds stay silent and preserve keyboard refresh", async ({
  page,
}) => {
  await startClient(page);
  await page.clock.install();
  await inDJSession(page);
  const button = page.getByRole("button", { name: "Refresh DJ", exact: true });
  await button.hover();
  await page.mouse.down();
  await page.clock.runFor(200);
  await page.mouse.move(0, 0);
  await page.clock.runFor(600);
  await page.mouse.up();
  await expect(
    page.getByText(
      "DJ text and voice requests aren't available yet. Click DJ for a new selection.",
      { exact: true },
    ),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) =>
            entry.command === "player" && entry.args.request.command === "dj",
        ).length,
    ),
  ).toBe(0);
  await button.focus();
  await button.press("Enter");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (entry: any) =>
              entry.command === "player" && entry.args.request.command === "dj",
          ).length,
      ),
    )
    .toBe(1);
});

test("personalized Home shelves launch native mixes and DJ", async ({
  page,
}) => {
  await startClient(page, "home");
  await expect(
    page.getByRole("heading", { name: "New releases for you", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Recommended Stations", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Nothing here yet", { exact: true })).toHaveCount(
    0,
  );
  await page
    .getByRole("button", { name: "Play Daily Mix 1", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((command: any) => command.command === "player")
            .at(-1)?.args.request,
      ),
    )
    .toMatchObject({
      command: "play_home",
      context: "spotify:playlist:1123456789abcdefghijkl",
    });
  await page.getByRole("button", { name: "Start DJ", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((command: any) => command.command === "player")
            .at(-1)?.args.request.command,
      ),
    )
    .toBe("dj");
  await page.screenshot({ path: "target/diagnostics/home-preview.png" });
});

test("cached Home shows at once and refreshes when native audio connects", async ({
  page,
}) => {
  await startClient(page, "home");
  const homeCalls = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (command: any) => command.command === "home",
        ).length,
    );
  const before = await homeCalls();
  await page.evaluate(() => {
    const { emit } = (window as any).__clientTest;
    emit({ type: "native", ready: false, status: "Connecting native audio…" });
    emit({
      type: "library",
      playlists: [
        {
          id: "4123456789abcdefghijkl",
          name: "Road trip",
          images: [],
          owner: { id: "test-user" },
          collaborative: false,
        },
      ],
    });
    emit({
      type: "home",
      page: {
        account: "test-user",
        greeting: "",
        cached: true,
        sections: [
          {
            uri: "cached",
            title: "Cached shelf",
            subtitle: "",
            items: [
              {
                uri: "spotify:album:5123456789abcdefghijkl",
                route: "album:5123456789abcdefghijkl",
                name: "Cached album",
                subtitle: "Artist",
                images: [],
                kind: "album",
                playable: true,
                format: "",
              },
            ],
          },
        ],
      },
    });
  });
  await expect(
    page.getByRole("heading", { name: "Cached shelf", exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".home-quick-links").getByText("Road trip", { exact: true }),
  ).toBeVisible();
  expect(await homeCalls()).toBe(before);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "native",
      ready: true,
      status: "Ready · this computer",
    }),
  );
  await expect.poll(homeCalls).toBe(before + 1);
  await expect(
    page.getByRole("heading", { name: "Daily Mixes", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Cached shelf", exact: true }),
  ).toHaveCount(0);
});

test("Home refreshes fresh-looking cached shelves on return, not on song changes", async ({
  page,
}) => {
  await startClient(page, "home");
  await expect(
    page.getByRole("heading", { name: "New releases for you", exact: true }),
  ).toBeVisible();
  const homeCalls = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "home",
        ).length,
    );
  const before = await homeCalls();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const invoke = (window as any).__TAURI_INTERNALS__.invoke;
    const album = {
      uri: "spotify:album:2123456789abcdefghijkl",
      route: "album:2123456789abcdefghijkl",
      name: "Latest release",
      subtitle: "Your artist",
      images: [],
      kind: "album",
      playable: true,
      format: "",
    };
    const mix = {
      ...album,
      uri: "spotify:playlist:1123456789abcdefghijkl",
      route: "playlist:1123456789abcdefghijkl",
      name: "Personal mix",
      kind: "playlist",
    };
    const artist = {
      ...album,
      uri: "spotify:artist:3123456789abcdefghijkl",
      route: "artist:3123456789abcdefghijkl",
      name: "Your artist",
      kind: "artist",
    };
    (window as any).__TAURI_INTERNALS__.invoke = async (
      command: string,
      args: any,
    ) => {
      if (command !== "home") return invoke(command, args);
      host.commands.push({ command, args });
      host.emit({
        type: "home",
        page: {
          account: "test-user",
          greeting: "Hello",
          cached: false,
          sections: [
            {
              uri: "spotlight",
              title: "New release from Your artist",
              subtitle: "",
              items: [album],
            },
            {
              uri: "jump-back",
              title: "Jump back in",
              subtitle: "",
              items: [artist, mix],
            },
            {
              uri: "recent",
              title: "Recents",
              subtitle: "",
              items: [mix, album],
            },
            {
              uri: "made-for",
              title: "Made For Listener",
              subtitle: "",
              items: [mix, album],
            },
          ],
        },
      });
    };
  });
  await page
    .locator(".home-quick-links")
    .getByRole("button", { name: "Liked Songs", exact: true })
    .click();
  await page.getByTitle("Home", { exact: true }).click();
  await expect.poll(homeCalls).toBe(before + 1);
  await expect(page.locator(".home-pair")).toContainText("New release from");
  await expect(page.locator(".home-pair")).toContainText("Jump back in");
  await expect(
    page.getByRole("heading", { name: "Recents", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Listener", exact: true }),
  ).toBeVisible();
  await page.evaluate(
    ({ playback, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: {
            ...track,
            uri: "spotify:track:2123456789abcdefghijkl",
            id: "2123456789abcdefghijkl",
          },
          is_playing: true,
        },
        observed_at: Date.now(),
        changed: true,
      }),
    { playback: emptyState.playback, track },
  );
  // Skipping while Home is open leaves its shelves alone, as in Spotify.
  await expect(page.locator(".player-bar")).toContainText(track.name);
  await page.waitForTimeout(300);
  expect(await homeCalls()).toBe(before + 1);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toBeVisible();
  await page.evaluate(
    ({ playback, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: {
            ...track,
            name: "Later song",
            uri: "spotify:track:3123456789abcdefghijkl",
            id: "3123456789abcdefghijkl",
          },
          is_playing: true,
        },
        observed_at: Date.now(),
        changed: true,
      }),
    { playback: emptyState.playback, track },
  );
  await expect(page.locator(".lyrics-page")).toContainText("Later song");
  expect(await homeCalls()).toBe(before + 1);
  await page.getByTitle("Home", { exact: true }).click();
  await expect.poll(homeCalls).toBe(before + 2);
});

test("big lists show their exact total length", async ({ page }) => {
  await startClient(page, "home");
  await page.evaluate(() => {
    (window as any).__clientTest.listDuration = [5000, 1_000_000_000];
  });
  await page
    .getByRole("button", { name: "Open Daily Mix 1", exact: true })
    .click();
  await expect(page.locator(".track-row")).toHaveCount(50);
  await page.evaluate(() => {
    const request = (window as any).__clientTest.commands
      .filter((request: any) => request.command === "browse")
      .at(-1);
    (window as any).__clientTest.emit({
      type: "browse_info",
      generation: request.args.generation,
      info: {
        key: request.args.key,
        title: "Daily Mix 1",
        subtitle: "",
        images: [],
        albums: [],
        playlist: { owner: "spotify", format: "", total_tracks: 5000 },
      },
    });
  });
  await expect(page.locator(".playlist-meta")).toContainText(
    "5000 songs, 277 hr 46 min",
  );
});

test("Daily Mix navigation retains title, artwork and description", async ({
  page,
}) => {
  await startClient(page, "home");
  await page
    .getByRole("button", { name: "Open Daily Mix 1", exact: true })
    .click();
  await expect(page.locator(".hero-copy h1")).toHaveText("Daily Mix 1");
  await expect(page.locator(".page-hero")).toContainText(
    "Your favorite artists",
  );
  await expect(page.locator(".page-hero img")).toHaveAttribute(
    "src",
    "https://artwork.example.test/cover.svg",
  );
  await expect(page.locator(".track-row")).toHaveCount(50);
  await page.evaluate(() => {
    const request = (window as any).__clientTest.commands
      .filter((request: any) => request.command === "browse")
      .at(-1);
    (window as any).__clientTest.emit({
      type: "browse_info",
      generation: request.args.generation,
      info: {
        key: request.args.key,
        title: "Daily Mix 1",
        subtitle: "Your favorite artists",
        images: [],
        albums: [],
        playlist: { owner: "spotify", format: "daily-mix", total_tracks: 50 },
      },
    });
  });
  await expect(page.locator(".playlist-meta")).toContainText(
    "Made for test-user",
  );
  await expect(page.locator(".playlist-meta")).toContainText("50 songs");
  await page.getByTitle("Go back", { exact: true }).click();
  await page
    .getByRole("button", { name: "Open Daily Mix 1", exact: true })
    .click();
  await expect(page.locator(".hero-copy h1")).toHaveText("Daily Mix 1");
});

test("Home shelves keep header, player and DJ inside the window", async ({
  page,
}) => {
  await startClient(page, "home");
  await page.evaluate(() => {
    const items = Array.from({ length: 25 }, (_, index) => ({
      uri: `spotify:playlist:${String(index).padStart(22, "0")}`,
      route: `playlist:${String(index).padStart(22, "0")}`,
      name: `Personal mix ${index + 1}`,
      subtitle: "Favorite artists and new discoveries",
      images: [],
      kind: "playlist",
      playable: true,
      format: "daily-mix",
    }));
    (window as any).__clientTest.emit({
      type: "home",
      page: {
        account: "test-user",
        greeting: "Hello",
        sections: [{ uri: "mixes", title: "Daily Mixes", subtitle: "", items }],
      },
    });
  });
  await expect(page.locator(".home-card")).toHaveCount(25);
  for (const viewport of [
    { width: 2560, height: 1323 },
    { width: 1600, height: 1000 },
    { width: 900, height: 600 },
  ]) {
    await page.setViewportSize(viewport);
    for (const selector of [
      ".application",
      ".topbar",
      ".window-controls",
      ".player-bar",
      ".player-extras",
      ".home-quick-links",
      '[aria-label="Start DJ"]',
    ]) {
      const box = await page.locator(selector).boundingBox();
      expect(box, selector).not.toBeNull();
      expect(
        box!.x,
        `${selector} extends past left edge`,
      ).toBeGreaterThanOrEqual(-1);
      expect(
        box!.x + box!.width,
        `${selector} extends past right edge at ${viewport.width}px`,
      ).toBeLessThanOrEqual(viewport.width + 1);
    }
    const player = await page.locator(".player-bar").boundingBox();
    expect(player!.y + player!.height).toBeLessThanOrEqual(viewport.height + 1);
  }
  // One row clipped at the edge with a fade, like Spotify; Show all wraps it.
  const shelf = page.locator(".home-shelf-items");
  await expect(shelf).toHaveClass(/clipped/);
  await page.getByRole("button", { name: "Show all", exact: true }).click();
  await expect(shelf).toHaveClass(/expanded/);
  await expect(shelf).not.toHaveClass(/clipped/);
  const width = await shelf.evaluate((element) => ({
    scroll: element.scrollWidth,
    client: element.clientWidth,
  }));
  expect(width.scroll).toBeLessThanOrEqual(width.client + 1);
  const footer = await page.locator(".player-bar").boundingBox();
  expect(footer!.y + footer!.height).toBeLessThanOrEqual(601);
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.getByRole("button", { name: "Show less", exact: true }).click();
  await page.screenshot({ path: "target/diagnostics/home-layout-fixed.png" });
});

test("imported lyrics keep artwork and ambience in place, without a duplicate preview", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from(
      '<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="0s" end="2s">Test</p></div></body></tt>',
    ),
  });
  await expect(page.locator(".lyrics-page .lyric-line")).toHaveCount(80);
  await expect(page.locator(".now-playing .lyric-preview")).toHaveCount(0);
  for (const position of [40_500, 88_500, 150_500]) {
    await page.evaluate(
      ({ uri, position }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms: position,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position },
    );
    const line = page.locator(".lyrics-page .lyric-line.active");
    await expect(line).toHaveText(
      `Test lyric line ${Math.floor(position / 2000) + 1}`,
    );
    await expect
      .poll(() =>
        page.evaluate(() => {
          const panel = document.querySelector<HTMLElement>(".lyrics-page")!;
          const ambience = panel.querySelector<HTMLElement>(".ambience")!;
          const body = panel.querySelector<HTMLElement>(".lyrics-body")!;
          const active =
            panel.querySelector<HTMLElement>(".lyric-line.active")!;
          const bounds = panel.getBoundingClientRect(),
            line = active.getBoundingClientRect();
          return (
            panel.scrollTop === 0 &&
            body.clientHeight <= panel.clientHeight &&
            Math.abs(ambience.getBoundingClientRect().top - bounds.top) < 1 &&
            line.top >= bounds.top &&
            line.bottom <= bounds.bottom
          );
        }),
      )
      .toBe(true);
  }
  await page.locator("footer").getByTitle("Lyrics", { exact: true }).click();
  const cards = page.locator(".now-playing .now-card");
  await expect(cards.first()).toHaveClass(/lyric-preview/);
  await expect(cards.nth(1)).toHaveClass(/artist-card/);
  // Card lyrics scale with the card and stay readable.
  const size = await cards
    .first()
    .locator(".lyric-line")
    .first()
    .evaluate((line) => parseFloat(getComputedStyle(line).fontSize));
  expect(size).toBeGreaterThanOrEqual(26);
  expect(size).toBeLessThanOrEqual(34);
});

test("album pages retain play counts, release date and licensing", async ({
  page,
}) => {
  await startClient(page);
  await page.locator(".album-link").first().click();
  await page.evaluate((track) => {
    const host = (window as any).__clientTest;
    const command = host.commands
      .filter((call: any) => call.command === "browse")
      .at(-1);
    host.emit({
      type: "browse_info",
      generation: command.args.generation,
      info: {
        key: command.args.key,
        title: "Test album",
        subtitle: "Test artist",
        images: track.album.images,
        album: {
          ...track.album,
          artists: track.artists,
          album_type: "SINGLE",
          total_tracks: 50,
          release_date: "2026-10-15",
          copyrights: [{ text: "© 2026 Test Label", type: "C" }],
        },
        albums: [],
        album_details: {
          streams: { "spotify:track:item0": 2470 },
          duration_ms: 180000,
          complete_duration: true,
          copyrights: [],
          more: [],
        },
      },
    });
  }, track);
  // Spotify sends "SINGLE"; the header reads "Single", like Spotify's.
  await expect(page.locator(".hero-copy .eyebrow")).toHaveText("Single");
  await expect(page.locator(".track-table-head")).toContainText("Plays");
  await expect(page.locator(".track-streams").first()).toHaveText("2,470");
  await expect(page.locator(".album-release-details")).toContainText(
    "2026 Test Label",
  );
  await expect(page.locator(".album-release-details")).toContainText("2026");
});

test("Now playing ambience follows its own Canvas, not the lyrics page's", async ({
  page,
}) => {
  await startClient(page);
  await emitCanvas(page);
  const ambience = page.locator(".now-playing > .ambience canvas");
  // Now playing shows the Canvas itself, so its ambience warps the video.
  await expect(
    page.locator(".now-playing video[data-canvas-video]"),
  ).toHaveCount(1);
  await expect(ambience).toHaveAttribute("data-source", "canvas");
  // With the Canvas on the lyrics page, the panel goes back to the artwork.
  await showLyrics(page);
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await expect(page.locator(".lyrics-canvas-cover video")).toHaveCount(1);
  await expect(
    page.locator(".now-playing video[data-canvas-video]"),
  ).toHaveCount(0);
  await expect(ambience).not.toHaveAttribute("data-source", "canvas");
});

test("right-clicking the Canvas in lyrics opens no song menu", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await emitCanvas(page);
  const video = page.locator(".lyrics-canvas-cover video");
  await expect(video).toBeVisible();
  const box = (await video.boundingBox())!;
  const x = box.x + Math.max(40, box.width * 0.3),
    y = box.y + Math.max(40, box.height * 0.3);
  expect(
    await page.evaluate(
      ([x, y]) => !!document.elementFromPoint(x, y)?.closest(".lyrics-album"),
      [x, y],
    ),
  ).toBe(true);
  await page.mouse.click(x, y, { button: "right" });
  await page.waitForTimeout(200);
  await expect(page.locator(".song-menu")).toHaveCount(0);
  // The title over it keeps its menu.
  await page
    .locator(".lyrics-page .lyrics-title")
    .click({ button: "right", force: true });
  await expect(page.locator(".song-menu")).toBeVisible();
});

test("Canvas defaults on, Space Gravity is optional, with sharp video left and metadata bottom-left", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await emitCanvas(page);
  await openSettings(page, "Effects");
  await expect(page.getByLabel("Lyrics API key")).toHaveCount(0);
  await expect(page.getByLabel("Canvas in lyrics")).toBeChecked();
  await expect(page.getByLabel("Space Gravity")).not.toBeChecked();
  await page.getByLabel("Canvas in lyrics").check();
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  await expect(page.locator(".canvas-lyrics video")).toBeVisible();
  await expect(page.locator(".gravity-stage")).toBeVisible();
  expect(
    await page
      .locator(".lyrics-canvas-cover video")
      .evaluate((element) => getComputedStyle(element).filter),
  ).toBe("none");
  expect(
    await page
      .locator(".lyrics-canvas-backdrop")
      .evaluate((element) => (element as HTMLCanvasElement).width),
  ).toBe(80);
  const layout = await page.evaluate(() => {
    const video = document
        .querySelector(".lyrics-canvas-cover")!
        .getBoundingClientRect(),
      body = document.querySelector(".lyrics-body")!.getBoundingClientRect(),
      title = document.querySelector(".lyrics-title")!.getBoundingClientRect(),
      section = document.querySelector(".lyrics-page")!.getBoundingClientRect();
    return {
      videoLeft:
        video.left <= section.left + 1 && video.width < section.width * 0.65,
      titleLeft: title.left < section.left + section.width / 2,
      titleBottom: title.top > section.top + section.height * 0.75,
    };
  });
  expect(layout).toEqual({
    videoLeft: true,
    titleLeft: true,
    titleBottom: true,
  });
  await openSettings(page, "Appearance");
  await page.getByLabel("Reduce motion").check();
  await showLyrics(page);
  // As in the fork, reduced motion keeps Space Gravity as still, centred
  // rows instead of drifting words.
  await expect(page.locator(".lyrics-canvas-cover")).toHaveCount(0);
  const still = page.locator(".gravity-word[data-spawned]").first();
  await expect(still).toBeVisible();
  await expect
    .poll(() => still.evaluate((node) => (node as HTMLElement).style.transform))
    .toContain("rotate(0deg)");
});

test("Canvas and its toolbar move together without restarting the video", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const toolbarBox = () =>
    page.evaluate(() => {
      const box = document
        .querySelector(".lyrics-toolbar")!
        .getBoundingClientRect();
      return { x: Math.round(box.left), y: Math.round(box.top) };
    });
  await expect(page.locator(".lyrics-album")).toBeVisible();
  const overCover = await toolbarBox();
  await emitCanvas(page);
  await expect(page.locator(".lyrics-canvas-cover video")).toBeVisible();
  // Canvas replacing the cover leaves the toolbar exactly where it was.
  await expect.poll(toolbarBox).toEqual(overCover);
  await page.evaluate(() => {
    (window as any).__canvasBeforeMove = document.querySelector(
      ".lyrics-canvas-cover video",
    );
  });
  const layout = () =>
    page.evaluate(() => {
      const section = document
        .querySelector(".lyrics-page")!
        .getBoundingClientRect();
      const cover = document
        .querySelector(".lyrics-canvas-cover")!
        .getBoundingClientRect();
      const toolbar = document
        .querySelector(".lyrics-toolbar")!
        .getBoundingClientRect();
      const title = document
        .querySelector(".lyrics-title")!
        .getBoundingClientRect();
      return {
        right: cover.left >= section.left + section.width / 2,
        anchorX: Math.abs(
          cover.left >= section.left + section.width / 2
            ? toolbar.right - (section.right - 16)
            : toolbar.left - (Math.max(section.left, cover.left) + 16),
        ),
        anchorY: Math.abs(
          toolbar.top - (Math.max(section.top, cover.top) + 14),
        ),
        contained:
          toolbar.right <= Math.min(section.right, cover.right) &&
          toolbar.bottom < Math.min(section.bottom, cover.bottom),
        titleRight: title.left > section.left + section.width / 2,
      };
    });
  await expect.poll(layout).toMatchObject({ right: false, contained: true });
  await expect.poll(async () => (await layout()).anchorX).toBeLessThan(2);
  await expect.poll(async () => (await layout()).anchorY).toBeLessThan(2);
  const flip = page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Artwork on the right", { exact: true });
  await flip.click();
  await expect
    .poll(layout)
    .toMatchObject({ right: true, contained: true, titleRight: true });
  await expect.poll(async () => (await layout()).anchorX).toBeLessThan(2);
  await expect.poll(async () => (await layout()).anchorY).toBeLessThan(2);
  expect(
    await page.evaluate(
      () =>
        (window as any).__canvasBeforeMove ===
        document.querySelector(".lyrics-canvas-cover video"),
    ),
  ).toBe(true);
  await flip.click();
  await expect
    .poll(layout)
    .toMatchObject({ right: false, contained: true, titleRight: false });
});

test("timed syllables lift independently and font choices apply separately", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "Hello",
            start_ms: 1000,
            end_ms: 3300,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "Hel", start_ms: 1000, end_ms: 1300 },
              { text: "lo", start_ms: 1300, end_ms: 3300 },
            ],
          },
        ],
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 1070,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  await expect(page.locator(".lyrics-page .lyric-syllable")).toHaveCount(2);
  const motion = (target: Locator) =>
    target.evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        lift:
          style.translate === "none"
            ? 0
            : parseFloat(style.translate.split(" ")[1] ?? "0"),
        scale: style.scale === "none" ? 1 : parseFloat(style.scale),
        glow: style.textShadow,
      };
    });
  const syllable = page.locator(".lyrics-page .lyric-syllable").first(),
    letter = page.locator(".lyrics-page .lyric-letter").first();
  // "Hel" is rising; the held "lo" rests as a whole word does, its letters
  // level and full size inside it, so it matches the word beside it.
  await expect.poll(async () => (await motion(syllable)).lift).toBeLessThan(0);
  await expect
    .poll(async () => Math.abs((await motion(letter)).lift))
    .toBeLessThan(0.001);
  await expect
    .poll(async () => Math.abs((await motion(letter)).scale - 1))
    .toBeLessThan(0.005);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 2000,
        playing: false,
        observed_at: Date.now(),
      }),
    track.uri,
  );
  await expect.poll(async () => (await motion(letter)).lift).toBeLessThan(-1);
  await expect
    .poll(async () => (await motion(letter)).scale)
    .toBeGreaterThan(1.02);
  expect((await motion(letter)).glow).toMatch(/rgba\(255, 255, 255, 0\.[1-9]/);
  // With no lyrics font chosen, song details keep the app font.
  expect(
    await page
      .locator(".lyrics-page .lyrics-title")
      .evaluate((element) => getComputedStyle(element).fontFamily),
  ).toContain("Metropolis");
  await openSettings(page, "Appearance");
  await page.getByLabel("App font", { exact: true }).fill("Georgia");
  await openSettings(page, "Lyrics");
  await page.getByLabel("Lyrics font", { exact: true }).fill("Consolas");
  await page.keyboard.press("Escape");
  await showLyrics(page);
  expect(
    await page
      .locator(".application")
      .evaluate((element) => getComputedStyle(element).fontFamily),
  ).toContain("Georgia");
  expect(
    await page
      .locator(".lyrics-page .lyric-line")
      .evaluate((element) => getComputedStyle(element).fontFamily),
  ).toContain("Consolas");
  expect(
    await page
      .locator(".lyrics-page .lyrics-title")
      .evaluate((element) => getComputedStyle(element).fontFamily),
  ).toContain("Consolas");
});

test("typed Windows font names apply immediately", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Appearance");
  const appFont = page.getByLabel("App font", { exact: true });
  const lyricFont = page.getByLabel("Lyrics font", { exact: true });
  await appFont.clear();
  await appFont.pressSequentially("Times New Roman", { delay: 10 });
  await expect(appFont).toHaveValue("Times New Roman");
  await expect(appFont).toBeFocused();
  expect(
    await page
      .locator(".application")
      .evaluate((node) => getComputedStyle(node).fontFamily),
  ).toContain("Times New Roman");
  await openSettings(page, "Lyrics");
  await lyricFont.fill("Bahnschrift");
  await expect(lyricFont).toBeFocused();
  await expect(page.locator(".lyrics-settings")).toBeVisible();
  expect(
    await page
      .locator(".application")
      .evaluate((node) =>
        getComputedStyle(node).getPropertyValue("--lyric-font"),
      ),
  ).toContain("Bahnschrift");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyric_font,
      ),
    )
    .toBe("Bahnschrift");
  await lyricFont.clear();
  expect(
    await page
      .locator(".application")
      .evaluate((node) =>
        getComputedStyle(node).getPropertyValue("--lyric-font"),
      ),
  ).toContain("Inter");
  await openSettings(page, "Appearance");
  await appFont.clear();
  expect(
    await page
      .locator(".application")
      .evaluate((node) => getComputedStyle(node).fontFamily),
  ).not.toContain("Times New Roman");
});

test("playlist search spans unloaded pages and play counts drive sorting", async ({
  page,
}) => {
  await startClient(page);
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  await page.getByTitle("Search in playlist", { exact: true }).click();
  await page.getByLabel("Search songs in this playlist").fill("Song 125");
  await expect(page.locator(".track-row")).toHaveCount(1);
  await expect(page.locator(".track-row")).toContainText("Song 125");
  await page.evaluate(() => {
    (window as any).__clientTest.emit({
      type: "track_facts",
      facts: {
        "spotify:track:item124": {
          plays: 123456,
          genres: ["rock"],
          release_date: "2020-01-01",
        },
      },
    });
  });
  await expect(page.locator(".track-streams")).toHaveCount(0);
  await page.getByLabel("Search songs in this playlist").clear();
  await page.getByTitle("Sort and filter", { exact: true }).click();
  await page.getByRole("button", { name: "Most played", exact: true }).click();
  await expect(page.locator(".track-row").first()).toContainText("Song 125");
  await page.getByLabel("Genre", { exact: true }).selectOption("rock");
  await expect(page.locator(".track-row")).toHaveCount(1);
});

test("Now playing and each page's sort are saved for the next launch", async ({
  page,
}) => {
  await startClient(page);
  const saved = (key: string) =>
    page.evaluate(
      (key) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings[key],
      key,
    );
  await page
    .locator("footer")
    .getByTitle("Now playing", { exact: true })
    .click();
  await expect.poll(() => saved("now_playing_open")).toBe(false);
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  await page.getByTitle("Sort and filter", { exact: true }).click();
  await page.getByRole("button", { name: "Most played", exact: true }).click();
  await expect.poll(() => saved("song_sorts")).toEqual({ liked: "popular" });
});

test("Random sort shuffles the playlist and reshuffles when picked again", async ({
  page,
}) => {
  await startClient(page);
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  const rows = page.locator(".track-row");
  await expect(rows.first()).toBeVisible();
  const names = () => rows.allInnerTexts();
  const original = await names();
  const sortButton = page.getByTitle("Sort and filter", { exact: true });
  const random = page
    .getByRole("dialog", { name: "Sort and filter" })
    .getByRole("button", { name: "Random", exact: true });
  await sortButton.click();
  await random.click();
  await expect(sortButton).toHaveText("Random");
  await expect.poll(names).not.toEqual(original);
  const shuffled = await names();
  await random.click();
  await expect.poll(names).not.toEqual(shuffled);
  // Leaving and coming back with Back keeps the order; opening it shuffles.
  await page.keyboard.press("Escape");
  const titles = () => rows.locator(".track-name").allInnerTexts();
  // Every song is loaded first, since more songs change the shuffle.
  let count = -1;
  await expect
    .poll(async () => {
      const previous = count;
      count = await rows.count();
      return count === previous;
    })
    .toBe(true);
  const kept = await titles();
  await page.locator(".home-button").click();
  await page.getByTitle("Go back", { exact: true }).click();
  await expect(rows.first()).toBeVisible();
  await expect.poll(titles).toEqual(kept);
  await page.locator(".home-button").click();
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  await expect(rows.first()).toBeVisible();
  await expect.poll(titles).not.toEqual(kept);
});

test("near-bottom scrolling automatically loads one page per offset", async ({
  page,
}) => {
  await startClient(page);
  await page.locator(".browse-scroll").evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    for (let i = 0; i < 20; i++) element.dispatchEvent(new Event("scroll"));
  });
  await expect(page.locator(".track-row")).toHaveCount(100);
  await page.locator(".browse-scroll").evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect(page.locator(".track-row")).toHaveCount(150);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands
        .filter(
          (entry: any) => entry.command === "browse" && entry.args.offset > 0,
        )
        .map((entry: any) => entry.args.offset),
    ),
  ).toEqual([50, 100]);
  await expect(
    page.getByRole("button", { name: "Load more songs" }),
  ).toHaveCount(0);
});

test("playlist toolbar stays above the track table", async ({ page }) => {
  await startClient(page);
  await page
    .getByRole("button", { name: "Liked Songs Playlist · Pinned" })
    .click();
  await expect(page.locator(".primary-play")).toBeVisible();
  await expect(page.locator(".browse-actions .track-table")).toHaveCount(0);
  const toolbar = await page.locator(".browse-actions").boundingBox();
  const rows = await page.locator(".track-row").first().boundingBox();
  expect(toolbar!.y + toolbar!.height).toBeLessThanOrEqual(rows!.y);
});

test("background-only vocals keep the main line anchored; long breaks show dots", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const line = (
      text: string,
      start_ms: number,
      end_ms: number,
      background = false,
    ) => ({
      text,
      start_ms,
      end_ms,
      background,
      opposite: false,
      words: [],
      translation: "",
      romanization: "",
    });
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "spicy_lyrics",
        synced: true,
        instrumental: false,
        attribution: {
          uploader: {
            id: "123456789012345678",
            username: "Uploader",
            url: "https://spicylyrics.org/uid/1",
          },
          maker: {
            id: "2",
            username: "! </Maker>",
            url: "https://spicylyrics.org/uid/2",
          },
        },
        lines: [
          line("Main line", 1000, 3000),
          line("Background echo", 1800, 6000, true),
          line("Next main line", 10000, 12000),
        ],
      },
    });
    (window as any).__clientTest.emit({
      type: "native_position",
      uri,
      position_ms: 2500,
      observed_at: Date.now(),
      playing: false,
    });
  }, track.uri);
  await expect(page.locator(".lyrics-page .lyric-line.active")).toHaveCount(2);
  // Wide lyrics start at 28% of their own viewport, independent of artwork.
  await expect
    .poll(() =>
      page.evaluate(() => {
        const line = document
            .querySelector(".lyrics-page .lyric-line")!
            .getBoundingClientRect(),
          view = document
            .querySelector(".lyrics-page .lyric-scroll")!
            .getBoundingClientRect();
        return (line.top - view.top) / view.height;
      }),
    )
    .toBeCloseTo(0.28, 2);
  const leadTop = await page
    .locator(".lyrics-page .lyric-line")
    .first()
    .evaluate((element) => element.getBoundingClientRect().top);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 4000,
        observed_at: Date.now(),
        playing: false,
      }),
    track.uri,
  );
  await expect(page.locator(".lyrics-page .lyric-line.active")).toHaveText(
    "Background echo",
  );
  await expect(
    page.getByRole("status", { name: "Instrumental break" }),
  ).toHaveCount(0);
  expect(
    await page
      .locator(".lyrics-page .lyric-line")
      .first()
      .evaluate((element) => element.getBoundingClientRect().top),
  ).toBeCloseTo(leadTop, 0);
  expect(
    await page
      .locator(".lyrics-page .lyric-wait")
      .first()
      .evaluate((element) => element.getBoundingClientRect().height),
  ).toBe(0);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 6500,
        observed_at: Date.now(),
        playing: false,
      }),
    track.uri,
  );
  await expect(
    page.getByRole("status", { name: "Instrumental break" }),
  ).toBeVisible();
  const dots = page.locator(".lyrics-page .lyric-wait[aria-hidden='false'] i");
  await expect(dots).toHaveCount(3);
  for (const dot of await dots.all()) await expect(dot).toBeVisible();
  await expect(
    page.locator(".lyrics-page .lyric-source.expanded"),
  ).toContainText("Community lyrics");
  await expect(
    page.locator(".lyrics-page .lyric-source.expanded"),
  ).toContainText("Uploaded by @Uploader");
  await expect(
    page.locator(".lyrics-page .lyric-source.expanded"),
  ).toContainText("Made by @! </Maker>");
  await page.route("https://spicylyrics.org/**", (route) =>
    route.fulfill({ contentType: "text/html", body: "<h1>Profile</h1>" }),
  );
  await page.getByRole("button", { name: "@Uploader", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.find(
            (call: any) => call.command === "open_creator_profile",
          )?.args,
      ),
    )
    .toEqual({ username: "Uploader", userId: "123456789012345678" });
  // A name a page path cannot hold still opens through the creator's id.
  await page.getByRole("button", { name: "@! </Maker>", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((call: any) => call.command === "open_creator_profile")
            .at(-1)?.args,
      ),
    )
    .toEqual({ username: "! </Maker>", userId: "2" });
  await expect(page.locator(".lyrics-album-name")).toHaveCount(0);
  expect(
    await page
      .locator(".lyrics-title")
      .evaluate((element) => getComputedStyle(element).textAlign),
  ).toBe("center");
});

test("lyrics wrap at narrow widths; countdown and unsung syllables follow song time", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const line = (
      text: string,
      start_ms: number,
      end_ms: number,
      words: any[] = [],
    ) => ({
      text,
      start_ms,
      end_ms,
      words,
      background: false,
      opposite: false,
      translation: "",
      romanization: "",
    });
    const words = [
      { text: "Long", start_ms: 10000, end_ms: 11000 },
      {
        text: " multi-part-word-with-many-parts-and-a-long-ending",
        start_ms: 11000,
        end_ms: 12000,
      },
      {
        text: " and a whole phrase with spaces that must wrap when the panel is small",
        start_ms: 12000,
        end_ms: 14000,
      },
    ];
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Local TTML",
        synced: true,
        instrumental: false,
        lines: [
          line("Before", 0, 3000),
          line(words.map((word) => word.text).join(""), 10000, 14000, words),
        ],
      },
    });
    (window as any).__clientTest.emit({
      type: "native_position",
      uri,
      position_ms: 3500,
      observed_at: Date.now(),
      playing: false,
    });
  }, track.uri);
  const dots = page.locator(".lyric-wait i");
  await expect(dots).toHaveCount(3);
  const initial = await dots
    .first()
    .evaluate((element) => Number(getComputedStyle(element).opacity));
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 7500,
        observed_at: Date.now(),
        playing: false,
      }),
    track.uri,
  );
  await expect
    .poll(() =>
      dots
        .first()
        .evaluate((element) => Number(getComputedStyle(element).opacity)),
    )
    .toBeGreaterThan(initial);
  for (const width of [1000, 1440, 2560]) {
    await page.setViewportSize({ width, height: 900 });
    await expect
      .poll(() =>
        page
          .locator(".lyric-line")
          .last()
          .evaluate((element) => element.scrollWidth - element.clientWidth),
      )
      .toBeLessThanOrEqual(2);
  }
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 10001,
        observed_at: Date.now(),
        playing: false,
      }),
    track.uri,
  );
  // Unsung syllables carry no sung fill.
  expect(["", "-20%"]).toContain(
    await page
      .locator(".lyric-syllable")
      .last()
      .evaluate((element) =>
        element.style.getPropertyValue("--gradient-position"),
      ),
  );
  expect(
    await page
      .locator(".lyric-line.active")
      .evaluate((element) => getComputedStyle(element).transform),
  ).toBe("none");
});

test("seek drag released outside slider resumes the audio clock", async ({
  page,
}) => {
  await startClient(page);
  const slider = page.getByLabel("Seek position"),
    box = await slider.boundingBox();
  if (!box) throw Error("Missing seek slider");
  await page.mouse.move(box.x + box.width * 0.3, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.65, box.y - 100);
  await page.mouse.up();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (call: any) =>
              call.command === "player" && call.args.request.command === "seek",
          ).length,
      ),
    )
    .toBeGreaterThan(0);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 62000,
        playing: false,
        observed_at: Date.now(),
        seeked: true,
      }),
    track.uri,
  );
  await expect(slider).toHaveValue("62000");
});
test("fullscreen moves lyrics right and Escape restores normal layout", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "Test",
          synced: true,
          instrumental: false,
          lines: [
            {
              text: "Main vocal",
              start_ms: 0,
              end_ms: 2000,
              words: [],
              background: false,
              opposite: false,
            },
            {
              text: "Duet vocal",
              start_ms: 2000,
              end_ms: 4000,
              words: [],
              background: false,
              opposite: true,
            },
          ],
        },
      }),
    track.uri,
  );
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page .lyric-line.opposite")).toHaveCSS(
    "text-align",
    "right",
  );
  const flow = page.locator(".lyrics-page .lyric-scroll"),
    before = (await flow.boundingBox())!.x;
  await page
    .getByTitle(/screen/i)
    .first()
    .click();
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  // The backend switches full screen, so a maximized window is not first
  // restored to its smaller size by the page.
  const windowCalls = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .map((c: any) => c.command)
      .filter((c: string) => /window/.test(c)),
  );
  expect(windowCalls).toContain("window_fullscreen");
  expect(windowCalls.join(" ")).not.toMatch(/unmaximize|set_fullscreen/);
  // Full screen shows only the lyrics page, with its own controls.
  await expect(page.locator(".topbar")).toBeHidden();
  await expect(page.locator("footer.player-bar")).toBeHidden();
  const controls = page.locator(".lyrics-fullscreen-controls");
  await expect(controls).toBeVisible();
  await controls.getByTitle("Next song", { exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request.command,
      ),
    )
    .toBe("next");
  await expect
    .poll(() =>
      flow.evaluate((element) => {
        const root = element.closest(".lyrics-page")!.getBoundingClientRect();
        const text = element.querySelector(".lyric-line")!;
        return (text.getBoundingClientRect().left - root.left) / root.width;
      }),
    )
    .toBeGreaterThan(0.5);
  await page.keyboard.press("Escape");
  await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  await expect(page.locator(".topbar")).toBeVisible();
  await expect(controls).toBeHidden();
  await expect
    .poll(async () => Math.abs((await flow.boundingBox())!.x - before))
    .toBeLessThan(2);
});

test("song title resolves native album metadata before navigating", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate((track) => {
    const item = {
      ...track,
      album: { ...track.album, id: undefined, uri: undefined },
    };
    (window as any).__clientTest.emit({
      type: "playback",
      playback: {
        item,
        is_playing: false,
        progress_ms: 0,
        shuffle_state: false,
        repeat_state: "off",
        device: { name: "Lyssna", is_active: true },
      },
      observed_at: Date.now(),
      changed: true,
    });
  }, track);
  await page
    .locator("footer")
    .getByRole("button", { name: track.name, exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.find(
            (call: any) => call.command === "resolve_album",
          )?.args.uri,
      ),
    )
    .toBe(track.uri);
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (call: any) =>
            call.command === "browse" &&
            call.args.key === "album:abcdefghijklmnopqrstuv",
        ),
      ),
    )
    .toBe(true);
});

// Explicit track with 30 synced lines; the first line starts at 10 s.
async function playLyricTrack(page: Page, playing = true) {
  await page.evaluate((playing) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "library",
      playlists: Array.from({ length: 12 }, (_, index) => ({
        id: `${index}123456789abcdefghijk`.slice(0, 22),
        name: [
          "Certified Jammers",
          "Songs to be synced",
          "MY FAV GD SONGS",
          "The Scientist",
          "PARANOID",
          "CHING CHONG",
        ][index % 6],
        images: [{ url: "https://artwork.example.test/cover.svg" }],
        owner: { id: "x", display_name: "Stormanzani" },
        collaborative: false,
      })),
    });
    host.emit({
      type: "playback",
      changed: false,
      observed_at: Date.now(),
      playback: {
        is_playing: playing,
        progress_ms: 3000,
        shuffle_state: false,
        repeat_state: "off",
        device: {
          name: "Lyssna (test)",
          id: "local-device",
          is_active: true,
        },
        item: {
          id: "0123456789abcdefghijkl",
          uri: "spotify:track:0123456789abcdefghijkl",
          name: "Come & Go (with Marshmello)",
          explicit: true,
          artists: [
            { id: "a", name: "Juice WRLD" },
            { id: "b", name: "Marshmello" },
          ],
          album: {
            id: "0123456789abcdefghijkl",
            name: "Album",
            images: [{ url: "https://artwork.example.test/cover.svg" }],
          },
          duration_ms: 180000,
          is_local: false,
        },
      },
    });
    host.emit({
      type: "lyrics",
      uri: "spotify:track:0123456789abcdefghijkl",
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: Array.from({ length: 30 }, (_, index) => ({
          text: `I am just a lyric line number ${index + 1}`,
          start_ms: 10000 + index * 3000,
          end_ms: 13000 + index * 3000,
          translation: "",
          romanization: "",
          background: false,
          opposite: false,
          words: [],
        })),
      },
    });
  }, playing);
}
test("a small scroll resumes following while the current lyric stays on screen", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page, false);
  const position = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) => {
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        });
      },
      { uri: track.uri, position_ms },
    );
  await position(40_500);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const display = page.locator(".lyrics-page .lyrics-display"),
    flow = display.locator(".lyric-scroll");
  await expect
    .poll(() => flow.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(100);
  await flow.hover();
  await page.mouse.wheel(0, 60);
  await expect(display).toHaveAttribute("data-following", "false");
  // Like Spicy Lyrics, following waits out the scroll cooldown first.
  await position(40_600);
  await expect(display).toHaveAttribute("data-following", "false");
  await page.waitForTimeout(800);
  await position(40_700);
  await expect(display).toHaveAttribute("data-following", "true");
});

test("lyric browsing stays put until the directional return button is clicked", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page, false);
  const position = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) => {
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        });
      },
      { uri: track.uri, position_ms },
    );
  await position(40_500);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const display = page.locator(".lyrics-page .lyrics-display"),
    flow = display.locator(".lyric-scroll"),
    back = display.getByRole("button", { name: "Return to current lyric" });
  await expect
    .poll(() => flow.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(100);
  await expect
    .poll(() =>
      flow.evaluate((node) => {
        const view = node.getBoundingClientRect(),
          line = node
            .querySelector(".lyric-line.active")!
            .getBoundingClientRect();
        return Math.abs(
          line.top + line.height / 2 - view.top - view.height / 2,
        );
      }),
    )
    .toBeLessThan(3);
  await flow.hover();
  await page.mouse.wheel(0, 900);
  await expect(display).toHaveAttribute("data-following", "false");
  await page.waitForTimeout(500);
  const browsed = await flow.evaluate((node) => node.scrollTop);
  await page.waitForTimeout(3200);
  expect(
    Math.abs((await flow.evaluate((node) => node.scrollTop)) - browsed),
  ).toBeLessThan(2);
  await expect(back).toBeVisible();
  await expect(back.locator(".lucide-chevron-up")).toBeVisible();
  await back.screenshot({
    path: "target/diagnostics/lyrics-return-arrow.png",
    animations: "disabled",
  });
  await position(95_500);
  await expect(display).toHaveAttribute("data-following", "false");
  await expect(back.locator(".lucide-chevron-down")).toBeVisible();
  await back.click();
  await expect(display).toHaveAttribute("data-following", "true");
  await expect(back).toHaveCount(0);
  await expect
    .poll(() => flow.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(browsed + 100);
});

test("wide lyrics start above centre, centre playback and end above padded credits", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await playLyricTrack(page, false);
  const position = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) => {
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        });
      },
      { uri: track.uri, position_ms },
    );
  await position(10_500);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const flow = page.locator(".lyrics-page .lyric-scroll");
  const anchor = (centre = false) =>
    flow.evaluate((node, centre) => {
      const view = node.getBoundingClientRect(),
        line = node
          .querySelector(".lyric-line.active")!
          .getBoundingClientRect();
      return (
        (line.top + (centre ? line.height / 2 : 0) - view.top) / view.height
      );
    }, centre);
  await expect.poll(() => anchor()).toBeCloseTo(0.28, 1);
  await position(40_500);
  await expect.poll(() => anchor(true)).toBeCloseTo(0.5, 2);
  await position(98_500);
  await expect
    .poll(() =>
      flow.evaluate((node) =>
        Math.abs(node.scrollHeight - node.clientHeight - node.scrollTop),
      ),
    )
    .toBeLessThan(2);
  const ending = await flow.evaluate((node) => {
    const view = node.getBoundingClientRect(),
      source = node
        .querySelector(".lyric-end-credits .lyric-source")!
        .getBoundingClientRect(),
      last = node.querySelector(".lyric-line.active")!.getBoundingClientRect();
    return {
      gap: view.bottom - source.bottom,
      last: (last.top - view.top) / view.height,
    };
  });
  expect(ending.gap).toBeGreaterThanOrEqual(40);
  expect(ending.gap).toBeLessThanOrEqual(80);
  expect(ending.last).toBeGreaterThan(0.6);
  await page.screenshot({
    path: "target/diagnostics/lyrics-ending-spacing.png",
  });
});

test("backing vocals keep their smaller size and selected lyric font", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ uri, settings }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "settings",
        settings: { ...settings, lyric_font: "Arial" },
      });
      host.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "test",
          synced: true,
          instrumental: false,
          lines: [false, true].map((background) => ({
            text: background ? "Backing vocal" : "Lead vocal",
            start_ms: 0,
            end_ms: 60_000,
            background,
            opposite: false,
            words: [],
          })),
        },
      });
    },
    { uri: track.uri, settings: emptyState.settings },
  );
  await page.getByTitle("Lyrics", { exact: true }).click();
  for (const width of [1920, 1440, 1100]) {
    await page.setViewportSize({ width, height: 1080 });
    const styles = await page
      .locator(".lyrics-page .lyric-line")
      .evaluateAll((nodes) =>
        nodes.map((node) => {
          const style = getComputedStyle(node);
          return { size: parseFloat(style.fontSize), font: style.fontFamily };
        }),
      );
    expect(styles[1].size / styles[0].size).toBeGreaterThan(0.5);
    expect(styles[1].size / styles[0].size).toBeLessThan(0.7);
    expect(styles[1].font).toBe(styles[0].font);
    expect(styles[1].font).toContain("Arial");
  }
});

for (const route of [
  "liked",
  `playlist:${track.id}`,
  `album:${track.album.id}`,
]) {
  test(`song table headings line up with numbers and duration text in ${route.split(":")[0]}`, async ({
    page,
  }) => {
    await startClient(page, route);
    await page.evaluate(
      ({ route, track }) => {
        const host = (window as any).__clientTest;
        const generation = host.commands
          .filter(
            (entry: any) =>
              entry.command === "browse" && entry.args.key === route,
          )
          .at(-1).args.generation;
        host.emit({
          type: "browse_info",
          generation,
          info: {
            key: route,
            title: "Alignment test",
            subtitle: "",
            images: [],
            albums: [],
            album: route.startsWith("album:") ? track.album : undefined,
          },
        });
      },
      { route, track },
    );
    for (const width of [1920, 1200]) {
      await page.setViewportSize({ width, height: 1000 });
      const offsets = await page.locator(".track-table").evaluate((table) => {
        const center = (selector: string) => {
          const element = table.querySelector(selector)!;
          const range = document.createRange();
          range.selectNodeContents(element);
          const box = element.matches("svg")
            ? element.getBoundingClientRect()
            : range.getBoundingClientRect();
          return (box.left + box.right) / 2;
        };
        return {
          number: Math.abs(
            center(".track-table-head > span:first-child") -
              center(".track-number"),
          ),
          duration: Math.abs(
            center(".track-table-head svg") - center(".row-end > span"),
          ),
        };
      });
      expect(offsets.number).toBeLessThan(1);
      expect(offsets.duration).toBeLessThan(1);
    }
  });
}

test("Appearance pins the lyrics footer with Off, No Writers and Full modes", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page, false);
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "spicy_lyrics",
        synced: true,
        instrumental: false,
        songwriters: ["Writer One", "Writer Two"],
        attribution: { maker: { username: "TestMaker" } },
        lines: Array.from({ length: 30 }, (_, index) => ({
          text: `Footer lyric ${index + 1}`,
          start_ms: 10000 + index * 3000,
          end_ms: 13000 + index * 3000,
          words: [],
          background: false,
          opposite: false,
        })),
      },
    });
  }, track.uri);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const mode = async (value: string) => {
    await page.getByTitle("Lyssna settings", { exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Lyssna settings" });
    // Settings reopen where they were left; pick Lyrics each time.
    await dialog.getByRole("tab", { name: "Lyrics", exact: true }).click();
    const select = dialog.getByLabel("Pinned Lyrics Footer", { exact: true });
    await expect(select.locator("option")).toHaveText([
      "Off",
      "No Writers",
      "Full",
    ]);
    await select.selectOption(value);
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as any).__clientTest.commands
              .filter((entry: any) => entry.command === "save_settings")
              .at(-1)?.args.settings.lyrics_pinned_footer,
        ),
      )
      .toBe(value);
    await page.keyboard.press("Escape");
  };
  const display = page.locator(".lyrics-page .lyrics-display"),
    flow = display.locator(".lyric-scroll"),
    pinned = display.getByRole("region", { name: "Pinned lyrics footer" });
  await expect(pinned).toHaveCount(0);
  await mode("no_writers");
  await expect(pinned).toContainText("Community lyrics");
  await expect(pinned).toContainText("@TestMaker");
  await expect(pinned).not.toContainText("Written by");
  // Readable and clear of the bottom edge, like Spicy Lyrics' footer.
  const footer = await pinned.evaluate((node) => {
    const source = node.querySelector(".lyric-source")!;
    const text = source.getBoundingClientRect();
    const box = node.closest(".lyrics-display")!.getBoundingClientRect();
    return {
      size: parseFloat(getComputedStyle(source).fontSize),
      gap: box.bottom - text.bottom,
    };
  });
  expect(footer.size).toBeGreaterThanOrEqual(13);
  expect(footer.gap).toBeGreaterThanOrEqual(30);
  await expect(flow.locator(".lyric-end-credits")).toContainText(
    "Written by Writer One, Writer Two",
  );
  await expect(flow.locator(".lyric-source")).toHaveCount(0);
  const before = (await pinned.boundingBox())!;
  await flow.hover();
  await page.mouse.wheel(0, 1400);
  await expect(display).toHaveAttribute("data-following", "false");
  await flow.evaluate((node) => {
    node.scrollTop = node.scrollHeight;
  });
  expect(Math.abs((await pinned.boundingBox())!.y - before.y)).toBeLessThan(1);
  const button = (await display
    .getByRole("button", { name: "Return to current lyric" })
    .boundingBox())!;
  expect(button.y + button.height).toBeLessThan(before.y);
  await mode("full");
  await expect(pinned).toContainText("Written by Writer One, Writer Two");
  await expect(pinned).toContainText("@TestMaker");
  await expect(flow.locator(".lyric-end-credits")).toHaveCount(0);
  // The lyrics font sets the writers line as well as the provider line.
  const fonts = await pinned.evaluate((node) => {
    (node as HTMLElement).style.setProperty(
      "--lyric-detail-font",
      '"Courier New"',
    );
    const family = (selector: string) =>
      getComputedStyle(node.querySelector(selector)!).fontFamily;
    return [family(".writer-credits"), family(".lyric-source")];
  });
  expect(fonts).toEqual(['"Courier New"', '"Courier New"']);
  await page.screenshot({ path: "target/diagnostics/pinned-lyrics-full.png" });
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 40_500,
        playing: false,
        observed_at: Date.now(),
      }),
    track.uri,
  );
  await display
    .getByRole("button", { name: "Return to current lyric" })
    .click();
  await expect
    .poll(() =>
      flow.evaluate((node) => {
        const box = node.getBoundingClientRect(),
          line = node
            .querySelector(".lyric-line.active")!
            .getBoundingClientRect();
        return Math.abs(line.top + line.height / 2 - box.top - box.height / 2);
      }),
    )
    .toBeLessThan(4);
  await page.setViewportSize({ width: 1100, height: 760 });
  await expect(pinned).toBeVisible();
  expect((await flow.boundingBox())!.height).toBeGreaterThan(100);
  await mode("off");
  await expect(pinned).toHaveCount(0);
  await expect(flow.locator(".lyric-end-credits")).toContainText("Writer One");
  await expect(flow.locator(".lyric-end-credits")).toContainText("@TestMaker");
});

test("stacked lyrics centre the active line in the lyric view", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1200, height: 760 });
  await startClient(page);
  await playLyricTrack(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "native_position",
      uri: "spotify:track:0123456789abcdefghijkl",
      position_ms: 31_500,
      playing: true,
      observed_at: Date.now(),
    }),
  );
  await page.getByTitle("Lyrics", { exact: true }).click();
  const offset = () =>
    page.evaluate(() => {
      const view = document
        .querySelector(".lyrics-page .lyric-scroll")!
        .getBoundingClientRect();
      const line = document
        .querySelector(".lyrics-page .lyric-line.active")!
        .getBoundingClientRect();
      return Math.abs(
        line.top + line.height / 2 - (view.top + view.height / 2),
      );
    });
  await expect.poll(offset, { timeout: 5_000 }).toBeLessThan(4);
});

test("explicit badges centre on the capitals beside them", async ({ page }) => {
  await startClient(page);
  await playLyricTrack(page);
  const offsets = await page.evaluate(() =>
    [...document.querySelectorAll(".explicit-badge")].map((badge) => {
      const link = badge.parentElement!.querySelector(".artist-link")!;
      const marker = document.createElement("span");
      marker.style.cssText =
        "display:inline-block;width:0;height:0;vertical-align:baseline";
      link.after(marker);
      const baseline = marker.getBoundingClientRect().top;
      marker.remove();
      const context = document.createElement("canvas").getContext("2d")!;
      context.font = getComputedStyle(link).font;
      const cap = context.measureText("H").actualBoundingBoxAscent;
      const box = badge.getBoundingClientRect();
      return Math.abs((box.top + box.bottom) / 2 - baseline + cap / 2);
    }),
  );
  expect(offsets.length).toBeGreaterThan(1);
  for (const offset of offsets) expect(offset).toBeLessThan(1);
});

test("DJ lyrics show the DJ cover and explain missing lyrics", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "narration", active: true }),
  );
  await page.getByTitle("Lyrics", { exact: true }).click();
  const lyrics = page.locator(".lyrics-page.dj-view");
  await expect(lyrics.locator(".lyrics-album .dj-art img")).toBeVisible();
  await expect(lyrics.locator(".ambience")).toHaveCount(1);
  await expect(lyrics).toContainText(
    "Lyrics aren't available while the DJ talks",
  );
  await expect(page.locator(".now-playing.dj-view .dj-art img")).toBeVisible();
});

test("Now playing lyric dots stay inside the lyrics card", async ({ page }) => {
  await startClient(page);
  await playLyricTrack(page);
  const card = await page.locator(".lyric-preview").boundingBox();
  const dots = page.locator(
    ".lyric-preview .lyric-wait:not([aria-hidden=true]) i",
  );
  await expect(dots).toHaveCount(3);
  for (const dot of await dots.all()) {
    const box = (await dot.boundingBox())!;
    expect(box.x).toBeGreaterThan(card!.x + 8);
    expect(box.y).toBeGreaterThan(card!.y + 40);
  }
  expect(
    await page
      .locator(".lyric-preview .lyric-wait:not([aria-hidden=true])")
      .evaluate((row) => getComputedStyle(row).overflow),
  ).toBe("visible");
});

test("the Now playing card fades its top only once lyrics scroll", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page);
  const scroller = page.locator(".lyric-preview .lyric-scroll");
  const fade = () =>
    scroller.evaluate((node) => ({
      top: node.scrollTop,
      mask: getComputedStyle(node).maskImage,
    }));
  const atTop = await fade();
  expect(atTop.top).toBe(0);
  expect(atTop.mask).toContain("min(0px");
  await scroller.evaluate((node) => {
    node.scrollTop = 120;
    node.dispatchEvent(new Event("scroll"));
  });
  await expect
    .poll(async () => (await fade()).mask)
    .toMatch(/min\((1[0-9]{2}|[1-9][0-9])px/);
});

test("the lyrics ambience rebuilds after the GPU loses its canvas", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await startClient(page);
  await playLyricTrack(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  const layer = page.locator(".lyrics-page > .ambience .ambient-layer");
  await expect(layer).toHaveCount(1);
  await layer.evaluate((canvas: HTMLCanvasElement) => {
    (canvas as any).__old = true;
    const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
    gl!.getExtension("WEBGL_lose_context")!.loseContext();
  });
  // A fresh canvas takes over, with a working context.
  await expect
    .poll(() =>
      layer.evaluate(
        (canvas: HTMLCanvasElement) =>
          !(canvas as any).__old &&
          !(
            canvas.getContext("webgl2") ?? canvas.getContext("webgl")
          )?.isContextLost(),
      ),
    )
    .toBe(true);
  expect(errors).toEqual([]);
});

test("Now playing retains the lyrics ambience with Liquid Glass enabled", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.keyboard.press("Escape");
  await openSettings(page, "Appearance");
  await page.getByRole("switch", { name: "Liquid Glass" }).check();
  await showLyrics(page);
  await expect(
    page.locator(".lyrics-page > .ambience .ambient-layer"),
  ).toHaveCount(1);
  await expect(
    page.locator(".now-playing > .ambience .ambient-layer"),
  ).toHaveCount(1);
  await expect(
    page.locator(".now-playing > .ambience .ambient-layer"),
  ).toBeVisible();
  await openSettings(page, "Appearance");
  await page.getByRole("switch", { name: "Liquid Glass" }).uncheck();
  await showLyrics(page);
  await expect(
    page.locator(".now-playing > .ambience .ambient-layer"),
  ).toHaveCount(1);
});

test("a 1440px laptop with Now playing open keeps side-by-side lyrics", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toBeVisible();
  await expect(page.locator(".lyrics-page")).not.toHaveClass(/narrow/);
});

test("narrow Canvas lyrics stack the title above the lyrics", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1300, height: 820 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await emitCanvas(page);
  await expect(page.locator(".lyrics-page.narrow.canvas-lyrics")).toBeVisible();
  await expect(page.locator(".lyrics-canvas-cover video")).toBeVisible();
  const cover = (await page.locator(".lyrics-canvas-cover").boundingBox())!;
  expect(cover.width).toBeCloseTo(56, 0);
  const artists = (await page
    .locator(".lyrics-page .lyrics-artists")
    .boundingBox())!;
  const lyrics = (await page
    .locator(".lyrics-page .lyric-scroll")
    .boundingBox())!;
  expect(artists.y + artists.height).toBeLessThanOrEqual(lyrics.y + 1);
  await expect
    .poll(
      () =>
        page.evaluate(() => {
          const view = document
            .querySelector(".lyrics-page .lyric-scroll")!
            .getBoundingClientRect();
          const line = document
            .querySelector(".lyrics-page .lyric-line.active")!
            .getBoundingClientRect();
          return Math.abs(
            line.top + line.height / 2 - (view.top + view.height / 2),
          );
        }),
      { timeout: 5_000 },
    )
    .toBeLessThan(4);
  await page.screenshot({ path: "target/diagnostics/canvas-narrow.png" });
});

test("compact lyrics centre the title and artists against cover and Canvas thumbnails", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Compact lyrics", { exact: true })
    .click();
  const alignment = () =>
    page.evaluate(() => {
      const root = document.querySelector(".lyrics-page")!;
      const cover = root
        .querySelector(".lyrics-album .cover")!
        .getBoundingClientRect();
      const title = root
        .querySelector(".lyrics-title")!
        .getBoundingClientRect();
      const artists = root
        .querySelector(".lyrics-artists")!
        .getBoundingClientRect();
      return Math.abs(
        (title.top + artists.bottom) / 2 - (cover.top + cover.bottom) / 2,
      );
    });
  await expect.poll(alignment).toBeLessThan(1);
  await emitCanvas(page);
  await expect(page.locator(".lyrics-canvas-cover video")).toBeVisible();
  await expect.poll(alignment).toBeLessThan(1);
});

test("unsung split words keep their syllables together", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const line = (start: number, words: [string, number, number][]) => ({
      text: words.map(([text]) => text).join(""),
      start_ms: start,
      end_ms: words.at(-1)![2],
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
      words: words.map(([text, start_ms, end_ms]) => ({
        text,
        start_ms,
        end_ms,
      })),
    });
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          line(1000, [["Ladies", 1000, 1600]]),
          line(5000, [
            ["gen", 5000, 5300],
            ["tle", 5300, 5600],
            ["men", 5600, 6600],
          ]),
        ],
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 1200,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  const pieces = page.locator(".lyrics-page .lyric-line").nth(1);
  await expect(pieces.locator(".lyric-syllable")).toHaveCount(3);
  const boxes = await pieces
    .locator(".lyric-syllable")
    .evaluateAll((elements) =>
      elements.map((element) => element.getBoundingClientRect().toJSON()),
    );
  // Each piece rests small about its own joint, so joints open a pixel at
  // most, and every piece rests at the same size and baseline.
  for (let index = 1; index < boxes.length; index++)
    expect(Math.abs(boxes[index].left - boxes[index - 1].right)).toBeLessThan(
      1.5,
    );
  const glyphs = await pieces
    .locator(".lyric-syllable")
    .evaluateAll((elements) =>
      elements.map((element) => {
        const box = element.getBoundingClientRect();
        return [box.height, box.bottom];
      }),
    );
  for (const [height, bottom] of glyphs) {
    expect(Math.abs(height - glyphs[0][0])).toBeLessThan(0.05);
    expect(Math.abs(bottom - glyphs[0][1])).toBeLessThan(0.05);
  }
  // Lines not yet reached rest at the size the animator holds unsung words
  // at. A split first word turns on its joint, as it does once the line
  // plays, so it sits a few pixels in from the margin both before and after.
  const lefts = await page
    .locator(".lyrics-page .lyric-line .lyric-main")
    .evaluateAll((elements) =>
      elements.map(
        (element) =>
          element.querySelector(".lyric-syllable")!.getBoundingClientRect()
            .left,
      ),
    );
  expect(Math.abs(lefts[0] - lefts[1])).toBeLessThan(4);
  // While the word is sung, one piece pops as the next rests small; no seam
  // opens between them.
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 5150,
        playing: false,
        observed_at: Date.now(),
      }),
    track.uri,
  );
  await expect(pieces).toHaveClass(/active/);
  await page.waitForTimeout(400);
  const sung = await pieces
    .locator(".lyric-syllable")
    .evaluateAll((elements) =>
      elements.map((element) => element.getBoundingClientRect().toJSON()),
    );
  for (let index = 1; index < sung.length; index++)
    expect(Math.abs(sung[index].left - sung[index - 1].right)).toBeLessThan(
      1.5,
    );
});

test("restarting a song slides lyrics back to the start instead of crawling", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from(
      '<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="0s" end="2s">Test</p></div></body></tt>',
    ),
  });
  await expect(page.locator(".lyrics-page .lyric-line")).toHaveCount(80);
  const seek = (position: number) =>
    page.evaluate(
      ({ uri, position }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms: position,
          playing: false,
          seeked: true,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position },
    );
  const scrollTop = () =>
    page
      .locator(".lyrics-page .lyric-scroll")
      .evaluate((element) => element.scrollTop);
  await seek(150_500);
  await expect.poll(scrollTop).toBeGreaterThan(1000);
  await page.waitForTimeout(1500);
  await seek(500);
  await expect(page.locator(".lyrics-page .lyric-line.active")).toHaveText(
    "Test lyric line 1",
  );
  // It slides rather than snapping, and arrives within the short glide.
  const before = await scrollTop();
  await page.waitForTimeout(250);
  const midway = await scrollTop();
  expect(midway).toBeGreaterThan(50);
  expect(midway).toBeLessThan(before);
  await expect.poll(scrollTop, { timeout: 900 }).toBeLessThan(50);
});

test("clicking a lyric seeks once and leaves Space for play and pause", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from(
      '<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="0s" end="2s">Test</p></div></body></tt>',
    ),
  });
  const requests = () =>
    page.evaluate((): string[] =>
      (window as any).__clientTest.commands
        .filter((call: any) => call.command === "player")
        .map((call: any) => call.args.request.command),
    );
  await page.locator(".lyrics-page .lyric-line").nth(3).click();
  await expect.poll(requests).toContain("seek");
  const seeks = (await requests()).filter((name) => name === "seek").length;
  expect(
    await page.evaluate(() =>
      document.activeElement?.classList.contains("lyric-line"),
    ),
  ).toBe(false);
  await page.keyboard.press("Space");
  await expect
    .poll(async () => (await requests()).at(-1))
    .toMatch(/^(play|pause)$/);
  expect((await requests()).filter((name) => name === "seek")).toHaveLength(
    seeks,
  );
});

test("taskbar and media keys drive playback and see what is playing", async ({
  page,
}) => {
  await startClient(page);
  const commands = () =>
    page.evaluate((): { command: string; args: any }[] =>
      (window as any).__clientTest.commands.map((call: any) => ({
        command: call.command,
        args: call.args,
      })),
    );
  await expect
    .poll(async () =>
      (await commands())
        .filter((call) => call.command === "media_state")
        .map((call) => call.args.state.title),
    )
    .toContain("Timing test");
  const press = (action: string, position_ms?: number) =>
    page.evaluate(
      ({ action, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "media_control",
          action,
          position_ms,
        }),
      { action, position_ms },
    );
  const requests = async () =>
    (await commands())
      .filter((call) => call.command === "player")
      .map((call) => call.args.request);
  await press("next");
  await expect
    .poll(async () => (await requests()).at(-1)?.command)
    .toBe("next");
  await press("toggle");
  await expect
    .poll(async () => (await requests()).at(-1)?.command)
    .toMatch(/^(play|pause)$/);
  await press("seek", 42_000);
  await expect
    .poll(async () => (await requests()).at(-1))
    .toMatchObject({
      command: "seek",
      positionMs: 42_000,
    });
});

test("home quick tiles keep small square covers with a centred heart", async ({
  page,
}) => {
  await startClient(page, "home");
  const layout = await page.locator(".home-quick-links").evaluate((root) => {
    const box = (selector: string) =>
      root.querySelector(selector)!.getBoundingClientRect();
    const tiles = [...root.querySelectorAll(".quick-tile")].map(
      (tile) => tile.getBoundingClientRect().height,
    );
    const cover = box(".liked-cover"),
      heart = box(".liked-cover svg"),
      dj = box(".dj-cover");
    return {
      tiles,
      dj: [dj.width, dj.height],
      offset: [
        heart.left + heart.width / 2 - (cover.left + cover.width / 2),
        heart.top + heart.height / 2 - (cover.top + cover.height / 2),
      ],
    };
  });
  for (const height of layout.tiles) expect(height).toBeCloseTo(72, 0);
  expect(layout.dj).toEqual([72, 72]);
  for (const offset of layout.offset) expect(Math.abs(offset)).toBeLessThan(1);
});

test("the queue opens in the right panel in place of Now playing", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate((track) => {
    (window as any).__clientTest.emit({
      type: "playing_details",
      uri: track.uri,
      details: {
        artists: track.artists,
        queue: [
          { ...track, uri: "spotify:track:upnext", name: "Up next song" },
        ],
      },
    });
  }, track);
  // Let the page finish loading its rows (the mock serves three pages) so
  // the count below cannot race the last page.
  await expect(page.locator(".track-row")).toHaveCount(150);
  const rows = await page.locator(".track-row").count();
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const panel = page.locator(".queue-panel");
  await expect(panel).toBeVisible();
  await expect(page.locator(".now-playing:not(.queue-panel)")).toHaveCount(0);
  await expect(panel).toContainText("Up next song");
  await expect(panel).toContainText("Timing test");
  // The main area keeps its page.
  await expect(page.locator(".track-row")).toHaveCount(rows);
  await panel.getByText("Up next song").click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((call: any) => call.command === "player")
            .at(-1)?.args.request.uri,
      ),
    )
    .toBe("spotify:track:upnext");
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  await expect(panel).toHaveCount(0);
  await expect(page.locator(".now-playing")).toBeVisible();
});

test("the app dims while a creator profile sheet is open", async ({ page }) => {
  await startClient(page);
  const emit = (open: boolean) =>
    page.evaluate(
      (open) =>
        (window as any).__clientTest.emit({ type: "creator_profile", open }),
      open,
    );
  await emit(true);
  await expect(page.locator(".creator-profile-dim")).toBeVisible();
  await emit(false);
  await expect(page.locator(".creator-profile-dim")).toHaveCount(0);
});

test("a left click anywhere outside the song menu closes it", async ({
  page,
}) => {
  await startClient(page);
  for (const outside of [".player-bar", ".track-row >> nth=5"]) {
    await page
      .locator(".track-row .track-name")
      .first()
      .click({ button: "right" });
    await expect(page.getByRole("menu")).toBeVisible();
    const box = (await page.locator(outside).boundingBox())!;
    await page.mouse.click(box.x + 8, box.y + box.height / 2);
    await expect(page.getByRole("menu")).toHaveCount(0);
  }
});

test("profiles follow Spotify's layout and open people, artists and playlists", async ({
  page,
}) => {
  await startClient(page);
  const item = (kind: string, id: string, name: string, subtitle: string) => ({
    uri: `spotify:${kind}:${id}`,
    route: `${kind}:${id}`,
    name,
    subtitle,
    images: [{ url: "https://artwork.example.test/cover.svg" }],
    kind,
    playable: kind !== "user",
    format: "",
  });
  const make = item;
  const profiles = {
    "test-user": {
      id: "test-user",
      name: "Test Listener",
      images: [{ url: "https://artwork.example.test/cover.svg" }],
      public_playlists: 1,
      followers: 1,
      following: 2,
      is_current_user: true,
      sections: [
        {
          uri: "spotify:user:test-user:top-artists",
          title: "Top artists this month",
          subtitle: "Only visible to you",
          items: [
            make("artist", "0123456789abcdefghijkl", "Top Artist", "Artist"),
          ],
        },
        {
          uri: "spotify:user:test-user:playlists",
          title: "Public Playlists",
          subtitle: "",
          items: [
            make(
              "playlist",
              "1123456789abcdefghijkl",
              "Road Trip",
              "By Test Listener",
            ),
          ],
        },
        {
          uri: "spotify:user:test-user:followers",
          title: "Followers",
          subtitle: "",
          items: [make("user", "friend", "Friend", "Profile")],
        },
      ],
    },
    friend: {
      id: "friend",
      name: "Friend",
      images: [],
      public_playlists: 0,
      followers: 3,
      following: 0,
      is_current_user: false,
      is_following: false,
      allow_follows: true,
      sections: [],
    },
  };
  await page.evaluate((profiles) => {
    (window as any).__clientTest.userProfiles = profiles;
  }, profiles);
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "Profile" }).click();
  const hero = page.locator(".profile-hero");
  await expect(hero.locator("h1")).toHaveText("Test Listener");
  await expect(hero.locator(".profile-counts > *")).toHaveText([
    "1 Public Playlist",
    "1 Follower",
    "2 Following",
  ]);
  const top = page.locator('section[aria-label="Top artists this month"]');
  await expect(top.locator(".home-shelf-note")).toHaveText(
    "Only visible to you",
  );
  await expect(top.locator(".home-card.artist-card")).toHaveCount(1);
  await expect(
    page.locator('section[aria-label="Public Playlists"] .home-card'),
  ).toContainText("By Test Listener");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.screenshot({ path: "target/diagnostics/profile-page.png" });
  const friend = page.locator('section[aria-label="Followers"] .home-card');
  await expect(friend).toHaveClass(/artist-card/);
  await expect(friend.locator(".home-card-play")).toBeDisabled();
  await friend.getByRole("button", { name: "Friend", exact: true }).click();
  await expect(hero.locator("h1")).toHaveText("Friend");
  await expect(page.locator(".profile-empty")).toHaveText(
    "This profile has nothing public yet.",
  );
  const actions = page.locator(".profile-actions");
  await actions.getByRole("button", { name: "Follow", exact: true }).click();
  await expect(
    actions.getByRole("button", { name: "Following", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(hero.locator(".profile-counts > *").nth(1)).toHaveText(
    "4 Followers",
  );
  const followCall = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "follow_user")
      .at(-1),
  );
  expect(followCall.args).toEqual({ user: "friend", follow: true });
  await page.getByTitle("Go back", { exact: true }).click();
  await expect(hero.locator("h1")).toHaveText("Test Listener");
  await page
    .locator('section[aria-label="Top artists this month"]')
    .getByRole("button", { name: "Top Artist", exact: true })
    .click();
  const browse = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1),
  );
  expect(browse.args.key).toBe("artist:0123456789abcdefghijkl");
  await expect(page.locator(".profile-hero")).toHaveCount(0);
});

test("Spotify+ copy actions, folder choice and contributor credits", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.route("https://github.com/**", (route) =>
    route.fulfill({
      contentType: "image/svg+xml",
      body: '<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96"/>',
    }),
  );
  await startClient(page);
  const clipboard = () => page.evaluate(() => navigator.clipboard.readText());
  // Song menu: copy name and ID.
  await page
    .locator(".track-row .track-name")
    .first()
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Copy song & artist name" }).click();
  await expect.poll(clipboard).toBe("Song 1 - Test artist");
  await page
    .locator(".track-row .track-name")
    .first()
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Copy embed code" }).click();
  await expect.poll(clipboard).toContain("open.spotify.com/embed/track/");
  // The hide-menu toggles are gone; folder choice lives under Your Library.
  await openSettings(page, "Library");
  await expect(page.getByText("Hide Profile in the profile menu")).toHaveCount(
    0,
  );
  await expect(
    page
      .locator(".lyrics-settings-section", { hasText: "Your Library" })
      .getByText("Add to playlist folders"),
  ).toBeVisible();
  // Credits list the creator first, with GitHub avatars.
  await page.getByRole("button", { name: /About and credits/ }).click();
  const people = page.locator(".lyrics-settings .credit-person");
  await expect(people).toHaveCount(3);
  await expect(people.first()).toContainText("Arashii");
  await expect(people.first().locator("img")).toHaveAttribute(
    "src",
    "https://github.com/Stormanzanii.png?size=96",
  );
  await people.first().click();
  const link = await page.evaluate(
    () =>
      (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "open_link")
        .at(-1)?.args.url,
  );
  expect(link).toBe("https://github.com/Stormanzanii");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  await expect(page.getByRole("menuitem", { name: "Profile" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Settings" })).toBeVisible();
});

test("song and playlist menus fit when opened, moved and resized", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1400, height: 720 });
  await startClient(page);
  const fits = async (menu: Locator) => {
    await expect(menu).toBeVisible();
    await expect
      .poll(() =>
        menu.evaluate((element) => {
          const box = element.getBoundingClientRect();
          return Math.min(
            box.top,
            box.left,
            window.innerWidth - box.right,
            window.innerHeight - box.bottom,
          );
        }),
      )
      .toBeGreaterThanOrEqual(7.5);
    await expect(menu.getByRole("menuitem").last()).toBeInViewport();
  };
  const openAt = (target: Locator, x: number, y: number) =>
    target.evaluate(
      (element, position) =>
        element.dispatchEvent(
          new MouseEvent("contextmenu", {
            bubbles: true,
            cancelable: true,
            clientX: position.x,
            clientY: position.y,
          }),
        ),
      { x, y },
    );
  const title = page.locator(".player-song .song-title");
  const songs = page.getByRole("menu", { name: "Song actions" });
  await title.click({ button: "right" });
  await fits(songs);
  // Reposition an already-open menu without remounting it.
  await openAt(title, 1390, 710);
  await fits(songs);
  await page.setViewportSize({ width: 900, height: 600 });
  await fits(songs);
  await page.keyboard.press("Escape");

  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "library",
      playlists: [
        {
          id: "5123456789abcdefghijkl",
          name: "Road trip",
          images: [],
          owner: { id: "test-user" },
          collaborative: false,
        },
      ],
    });
    host.emit({
      type: "library_folders",
      folders: [{ id: "gym", name: "Gym", parents: [] }],
    });
  });
  const playlist = page.locator(".library-row", { hasText: "Road trip" });
  await openAt(playlist, 890, 590);
  const playlists = page.getByRole("menu", { name: "playlist actions" });
  await fits(playlists);
  await openAt(playlist, 40, 40);
  await fits(playlists);
  await expect
    .poll(() =>
      playlists.evaluate((element) => element.getBoundingClientRect().top),
    )
    .toBe(40);
  await page.keyboard.press("Escape");
  await expect(playlists).toHaveCount(0);
});

test("the avatar menu opens settings, profile and grouped recents", async ({
  page,
}) => {
  await startClient(page);
  const menu = () =>
    page.getByRole("button", { name: "Account menu", exact: true });
  await menu().click();
  await expect(page.getByRole("menu", { name: "Account" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "Account" })).toHaveCount(0);

  await menu().click();
  await page.getByRole("menuitem", { name: "Profile" }).click();
  await expect(page.locator(".profile-hero .eyebrow")).toHaveText("Profile");

  await menu().click();
  await page.getByRole("menuitem", { name: "Recents" }).click();
  const request = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1),
  );
  expect(request.args.key).toBe("recent");
  await page.evaluate((generation) => {
    const now = Date.now();
    const base = {
      artists: [{ id: "artistartistartistarti", name: "Artist" }],
      album: { name: "Record", images: [] },
      duration_ms: 1000,
      is_local: false,
    };
    (window as any).__clientTest.emit({
      type: "tracks",
      key: "recent",
      generation,
      offset: 0,
      more: false,
      tracks: [
        {
          ...base,
          uri: "spotify:track:one",
          name: "One",
          added_at: new Date(now - 60_000).toISOString(),
          played_context: "spotify:album:abcdefghijklmnopqrstuv",
        },
        {
          ...base,
          uri: "spotify:track:two",
          name: "Two",
          added_at: new Date(now - 120_000).toISOString(),
          played_context: "spotify:album:abcdefghijklmnopqrstuv",
        },
      ],
    });
  }, request.args.generation);
  await expect(page.locator(".recents-day h2").first()).toHaveText("Today");
  await expect(page.locator(".recents-group")).toHaveCount(1);
  await expect(page.locator(".recents-group")).toContainText("2 songs played");
  await page.getByTitle("Show songs").click();
  await expect(page.locator(".recents-tracks li")).toHaveCount(2);
});

test("Liquid Glass refracts the lyrics toolbar, profile menu and right-click menus", async ({
  page,
}) => {
  await startClient(page);
  await expect(page.locator("#glass-refraction")).toHaveCount(0);
  await openSettings(page, "Appearance");
  await expect(page.getByRole("switch", { name: "Refraction" })).toHaveCount(0);
  await page.getByRole("switch", { name: "Liquid Glass" }).check();
  await expect(page.locator("#glass-refraction")).toHaveCount(1);
  const backdrop = (selector: string) =>
    page
      .locator(selector)
      .first()
      .evaluate((node) => getComputedStyle(node).backdropFilter);
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  expect(await backdrop(".profile-menu")).toContain("glass-refraction");
  await page.keyboard.press("Escape");
  // Right-click menus refract too.
  await page
    .locator(".track-row .track-name")
    .first()
    .click({ button: "right" });
  expect(await backdrop(".song-menu")).toContain("glass-refraction");
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  expect(await backdrop(".lyrics-toolbar")).toContain("glass-refraction");
  expect(await backdrop(".library-panel")).not.toContain("glass-refraction");
  expect(
    await page
      .locator(".search-box")
      .evaluate((node) => getComputedStyle(node, "::before").backdropFilter),
  ).not.toContain("glass-refraction");
});

test("Liquid Glass is off by default and toggles from Settings", async ({
  page,
}) => {
  await startClient(page);
  await expect(page.locator(".application")).not.toHaveClass(/liquid-glass/);
  await openSettings(page, "Appearance");
  await page.getByRole("switch", { name: "Liquid Glass" }).check();
  await expect(page.locator(".application")).toHaveClass(/liquid-glass/);
  await expect(page.locator(".glass-backdrop")).toHaveCount(1);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.liquid_glass,
      ),
    )
    .toBe(true);
  await page.getByLabel("Search settings").fill("liquid glass");
  await expect(page.locator(".setting-row")).toHaveCount(1);
  await page.getByLabel("Search settings").fill("zzzz");
  await expect(page.locator(".lyrics-settings-empty")).toBeVisible();
});

test("Space Gravity waiting dots share the normal countdown and drift with words", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const line = (text: string, start_ms: number, end_ms: number) => ({
      text,
      start_ms,
      end_ms,
      words: [{ text, start_ms, end_ms }],
      background: false,
      opposite: false,
      translation: "",
      romanization: "",
    });
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [line("Before", 0, 1000), line("After", 10000, 12000)],
      },
    });
  }, track.uri);
  const position = async (position_ms: number, playing = false) =>
    page.evaluate(
      async ({ uri, position_ms, playing }) => {
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing,
          observed_at: Date.now(),
        });
        await new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        );
      },
      { uri: track.uri, position_ms, playing },
    );
  const dots = () =>
    page
      .locator(".lyrics-page .lyric-wait:not([aria-hidden=true]) i")
      .evaluateAll((nodes) =>
        nodes.map((node) => ({
          opacity: getComputedStyle(node).opacity,
          transform: getComputedStyle(node).transform,
        })),
      );
  const normal = new Map<number, Awaited<ReturnType<typeof dots>>>();
  for (const time of [2000, 6500, 9500]) {
    await position(time);
    normal.set(time, await dots());
  }
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  const waiting = page.locator(".gravity-wait");
  await expect(waiting.locator("i")).toHaveCount(3);
  for (const [time, expected] of normal) {
    await position(time);
    await expect.poll(dots).toEqual(expected);
  }
  await position(6500, true);
  const sample = () =>
    waiting.evaluate((node) => (node as HTMLElement).style.transform);
  await expect.poll(sample).toContain("translate3d");
  const samples = [await sample()];
  for (let index = 0; index < 3; index += 1) {
    await page.waitForTimeout(250);
    samples.push(await sample());
  }
  expect(new Set(samples).size).toBeGreaterThan(2);
  await position(10000);
  await expect(waiting).toHaveCount(0);
  await position(6500);
  await expect(waiting.locator("i")).toHaveCount(3);
  await expect.poll(dots).toEqual(normal.get(6500));
  await expect.poll(sample).toContain("translate3d");
});

test("Space Gravity shares normal syllable joins and held-letter timing", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "Hello na-na-na",
            start_ms: 1000,
            end_ms: 5700,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "Hel", start_ms: 1000, end_ms: 1300 },
              { text: "lo", start_ms: 1300, end_ms: 3300 },
              { text: " na-na-na", start_ms: 3300, end_ms: 5700 },
            ],
          },
        ],
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 3500,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  const letters = page.locator(".lyrics-page .lyric-letter");
  await expect(letters).toHaveCount(10);
  const fills = () =>
    letters.evaluateAll((nodes) =>
      nodes.map((node) =>
        (node as HTMLElement).style.getPropertyValue("--gradient-position"),
      ),
    );
  await expect.poll(async () => (await fills()).every(Boolean)).toBe(true);
  const position = async (position_ms: number) =>
    page.evaluate(
      async ({ uri, position_ms }) => {
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        });
        await new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        );
      },
      { uri: track.uri, position_ms },
    );
  const normalFills = new Map<number, string[]>();
  for (const time of [1200, 3500, 4900]) {
    await position(time);
    normalFills.set(time, await fills());
  }
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  await expect(page.locator(".gravity-word")).toHaveCount(4);
  await expect(letters).toHaveCount(10);
  for (const [time, expected] of normalFills) {
    await position(time);
    await expect.poll(fills).toEqual(expected);
  }
  // The sung line shows at full strength, as in the fork; its fill marks
  // the sung words.
  await expect
    .poll(() =>
      page
        .locator(".gravity-word.current")
        .first()
        .evaluate((node) => Number(getComputedStyle(node).opacity)),
    )
    .toBeCloseTo(1, 2);
  await expect(page.locator(".gravity-word .joined-start")).toHaveText("Hel");
  await expect(page.locator(".gravity-word .joined-end")).toHaveText("lo");
  await expect
    .poll(() =>
      letters
        .nth(2)
        .evaluate((node) =>
          (node as HTMLElement).style.getPropertyValue("--glow"),
        ),
    )
    .not.toBe("");
});

test("Highlighter inks each floating word in Space Gravity", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "Mmmmmmmmmm next",
            start_ms: 0,
            end_ms: 6000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "Mmmmmmmmmm", start_ms: 0, end_ms: 4000 },
              { text: " next", start_ms: 4000, end_ms: 6000 },
            ],
          },
        ],
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 2000,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  const word = page.locator(".gravity-word").first();
  await expect(word.locator(".highlighter-piece")).toHaveText("Mmmmmmmmmm");
  // Half the word is sung, so half its ink is down, however it has turned.
  const share = () =>
    word.evaluate((node) => {
      const band = node.querySelector<HTMLElement>(".highlighter-stroke");
      const piece = node.querySelector<HTMLElement>(".highlighter-piece")!;
      if (!band) return -1;
      const em = parseFloat(getComputedStyle(piece).fontSize);
      const inset = Number(
        /inset\(0px ([\d.]+)px/.exec(band.style.clipPath)?.[1] ?? NaN,
      );
      const shown = parseFloat(band.style.width) - inset - 0.16 * em;
      return shown / piece.offsetWidth;
    });
  await expect.poll(share).toBeGreaterThan(0.44);
  expect(await share()).toBeLessThan(0.56);
  // The next word has no ink yet.
  await expect(
    page.locator(".gravity-word").nth(1).locator(".highlighter-stroke"),
  ).toHaveCount(0);
  // Lead words drop the pen tip; the connecting lines carry the pen on.
  await expect(word.locator(".highlighter-tip")).toBeHidden();
});

test("Highlighter joins sung words with marker lines in Space Gravity", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  const at = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms },
    );
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "One two three",
            start_ms: 0,
            end_ms: 6000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "One", start_ms: 0, end_ms: 1000 },
              { text: " two", start_ms: 2000, end_ms: 3000 },
              { text: " three", start_ms: 4000, end_ms: 6000 },
            ],
          },
          {
            text: "Four five",
            start_ms: 7000,
            end_ms: 8500,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "Four", start_ms: 7000, end_ms: 7500 },
              { text: " five", start_ms: 8000, end_ms: 8500 },
            ],
          },
          {
            text: "Six seven",
            start_ms: 9000,
            end_ms: 10000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [
              { text: "Six", start_ms: 9000, end_ms: 9300 },
              { text: " seven", start_ms: 9500, end_ms: 10000 },
            ],
          },
          {
            text: "Eight",
            start_ms: 13000,
            end_ms: 14000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [{ text: "Eight", start_ms: 13000, end_ms: 14000 }],
          },
        ],
      },
    });
  }, track.uri);
  await at(500);
  const links = page.locator(".gravity-links path");
  await expect(page.locator(".gravity-word").first()).toBeVisible();
  // Nothing is joined until a word is sung.
  await expect(links).toHaveCount(0);
  // Halfway through the pause after "One", its line is half drawn; it
  // lands just before "two" starts.
  await at(1450);
  await expect(links).toHaveCount(1);
  const offset = () =>
    links
      .first()
      .evaluate((path) =>
        Number((path as SVGPathElement).style.strokeDashoffset),
      );
  await expect.poll(offset).toBeCloseTo(0.5, 2);
  // Once "two" starts, the line reaches it and stays.
  await at(2500);
  await expect.poll(offset).toBeCloseTo(0, 2);
  await at(4500);
  await expect(links).toHaveCount(2);
  // The link into the next line draws across the pause before it, rather
  // than appearing whole when that line starts.
  await at(6450);
  const into = page.locator('.gravity-links path[data-link="0:2>1:0"]');
  await expect
    .poll(() =>
      into.evaluate((path) =>
        Number((path as SVGPathElement).style.strokeDashoffset),
      ),
    )
    .toBeCloseTo(0.5, 1);
  // The line before the one being sung keeps its links...
  await at(7700);
  const kept = page.locator(".gravity-links path:not(.leaving)");
  // Two in the earlier line, one into the sung line, one within it.
  await expect(kept).toHaveCount(4);
  // A link from a dim word into the sung line is half dim, half bright,
  // each half matching its word's ink so the line reads as one stroke.
  const halves = () =>
    page.evaluate(() => {
      const path = document.querySelector<SVGPathElement>(
        '.gravity-links path[data-link="0:2>1:0"]',
      );
      const stops = document.querySelectorAll(
        `#${path?.dataset.gradient} stop`,
      );
      return [...stops].map((stop) =>
        Number(stop.getAttribute("stop-opacity")),
      );
    });
  await expect.poll(async () => (await halves())[0]).toBeCloseTo(0.22, 1);
  await expect.poll(async () => (await halves())[3]).toBeCloseTo(1, 1);
  await expect(into).toHaveCSS("opacity", "1");
  // ...and older lines' links fade out rather than vanish.
  await at(9600);
  await expect(kept).toHaveCount(3);
  await expect(page.locator(".gravity-links path.leaving")).not.toHaveCount(0);
  // A fading link keeps following its drifting words rather than freezing.
  const fading = page.locator(".gravity-links path.leaving").first();
  const shape = await fading.getAttribute("d");
  await expect.poll(() => fading.getAttribute("d")).not.toBe(shape);
  // No link ever runs to the corner, where a word not yet placed sits.
  const corners = await page.evaluate(
    () =>
      [...document.querySelectorAll(".gravity-links path")].filter((path) => {
        const numbers = (path.getAttribute("d") ?? "")
          .match(/-?[\d.]+/g)!
          .map(Number);
        const ends = [numbers.slice(0, 2), numbers.slice(-2)];
        return ends.some(([x, y]) => Math.hypot(x, y) < 5);
      }).length,
  );
  expect(corners).toBe(0);
  await expect
    .poll(() =>
      page
        .locator(".gravity-links path.leaving")
        .first()
        .evaluate((path) => getComputedStyle(path).transitionDuration),
    )
    .toBe("1.2s");
  // Over a break with the waiting dots, the link into the next line shows
  // brighter than its dim words, still half and half.
  await at(12000);
  const pause = () =>
    page.evaluate(() => {
      const path = document.querySelector<SVGPathElement>(
        '.gravity-links path[data-link="2:1>3:0"]',
      );
      const stops = document.querySelectorAll(
        `#${path?.dataset.gradient} stop`,
      );
      return [...stops].map((stop) =>
        Number(stop.getAttribute("stop-opacity")),
      );
    });
  await expect.poll(async () => (await pause())[0]).toBeGreaterThan(0.45);
  await expect
    .poll(async () => {
      const stops = await pause();
      return stops[3] - stops[0];
    })
    .toBeGreaterThan(0.15);
});

test("Space Gravity marker links leave each word once it is inked, at the Highlighter's width", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  const at = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms },
    );
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "Quick quick words",
            start_ms: 0,
            end_ms: 3000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            // Sung back to back, faster than a link takes to draw.
            words: [
              { text: "Quick", start_ms: 0, end_ms: 100 },
              { text: " quick", start_ms: 100, end_ms: 200 },
              { text: " words", start_ms: 200, end_ms: 3000 },
            ],
          },
        ],
      },
    });
  }, track.uri);
  await at(50);
  await expect(page.locator(".gravity-word")).toHaveCount(3);
  const links = page.locator(".gravity-links path");
  const offsets = () =>
    links.evaluateAll((paths) =>
      paths.map((path) =>
        Number((path as SVGPathElement).style.strokeDashoffset),
      ),
    );
  // A link never leaves a word still being inked: "Quick" must finish
  // first. Back to back, it still glides rather than snapping whole.
  await expect(links).toHaveCount(0);
  await at(100);
  await expect(links).toHaveCount(1);
  expect((await offsets())[0]).toBeGreaterThan(0.9);
  await at(150);
  await expect.poll(async () => (await offsets())[0]).toBeLessThan(0.95);
  expect((await offsets())[0]).toBeGreaterThan(0.05);
  await expect(links).toHaveCount(1);
  // It lands as "quick" ends, when the second link sets off, so the two
  // read as one line.
  await at(200);
  await expect.poll(async () => (await offsets())[0]).toBe(0);
  await expect(links).toHaveCount(2);
  await at(450);
  await expect.poll(offsets).toEqual([0, 0]);
  // As broad as the Highlighter's band on the words.
  const width = await links
    .first()
    .evaluate((path) => parseFloat((path as SVGPathElement).style.strokeWidth));
  const band = await page
    .locator(".gravity-word .highlighter-piece")
    .first()
    .evaluate((piece) => piece.getBoundingClientRect().height);
  expect(width).toBeGreaterThan(band * 0.5);
  // Effects can thin the lines, or turn them off.
  await openSettings(page, "Effects");
  await page
    .getByRole("slider", { name: "Connecting line thickness" })
    .fill("50");
  await page.keyboard.press("Escape");
  await expect
    .poll(() =>
      links
        .first()
        .evaluate((path) =>
          parseFloat((path as SVGPathElement).style.strokeWidth),
        ),
    )
    .toBeCloseTo(width / 2, 0);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Connecting lines" }).click();
  await page.keyboard.press("Escape");
  await expect(page.locator(".gravity-links path:not(.leaving)")).toHaveCount(
    0,
  );
  // Without the lines, words keep their pen tip.
  await expect(page.locator(".gravity-lyrics")).not.toHaveClass(/linked/);
  // The options only show with both Space Gravity and the Highlighter on.
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await expect(
    page.getByRole("switch", { name: "Connecting lines" }),
  ).toHaveCount(0);
});

test("Space Gravity words keep drifting while the song plays", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: true,
          progress_ms: 40_000,
          device: { name: "Lyssna", is_active: true },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  // The sung word stays on screen while sampled; older words may leave.
  const word = page.locator(".gravity-word.current:not(.exiting)").first();
  await expect(word).toBeVisible();
  const key = await word.textContent();
  const sample = () =>
    page
      .locator(".gravity-word:not(.exiting)", { hasText: key! })
      .first()
      .evaluate((element) => (element as HTMLElement).style.transform);
  await page.waitForTimeout(300);
  const samples = [await sample()];
  for (let index = 0; index < 3; index += 1) {
    await page.waitForTimeout(250);
    samples.push(await sample());
  }
  // Re-rendering every frame must not respawn words at their start point.
  expect(new Set(samples).size).toBeGreaterThan(2);
  // The sung line's fill sweeps smoothly like the normal lyrics view.
  const fill = () =>
    page
      .locator(".gravity-word.current .lyric-syllable")
      .evaluateAll((elements) =>
        elements.reduce(
          (sum, element) =>
            sum +
            (parseFloat(
              (element as HTMLElement).style.getPropertyValue(
                "--gradient-position",
              ),
            ) || 0),
          0,
        ),
      );
  const first = await fill();
  await page.waitForTimeout(250);
  expect(await fill()).toBeGreaterThan(first);
  await page.screenshot({ path: "target/diagnostics/space-gravity.png" });
});

test("Space plays and pauses after clicking any button, but not while searching", async ({
  page,
}) => {
  await startClient(page);
  const requests = () =>
    page.evaluate((): string[] =>
      (window as any).__clientTest.commands
        .filter((call: any) => call.command === "player")
        .map((call: any) => call.args.request.command),
    );
  for (const title of ["Lyrics", "Queue", "Lyrics"]) {
    await page.locator("footer").getByTitle(title, { exact: true }).click();
    const before = (await requests()).length;
    await page.keyboard.press("Space");
    await expect.poll(async () => (await requests()).length).toBe(before + 1);
    expect((await requests()).at(-1)).toMatch(/^(play|pause)$/);
  }
  // Space on a focused button must not also press that button.
  const lyricsOpen = await page.locator(".lyrics-page").count();
  await page.locator("footer").getByTitle("Lyrics", { exact: true }).focus();
  await page.keyboard.press("Space");
  await page.waitForTimeout(100);
  expect(await page.locator(".lyrics-page").count()).toBe(lyricsOpen);
  const before = (await requests()).length;
  await page.getByPlaceholder("What do you want to play?").click();
  await page.keyboard.press("Space");
  await page.waitForTimeout(100);
  expect((await requests()).length).toBe(before);
});

test("clicking outside the device popup closes it", async ({ page }) => {
  await startClient(page);
  const devices = page.locator("footer").getByTitle("Devices", { exact: true });
  await devices.click();
  await expect(page.locator(".device-menu")).toBeVisible();
  await page.locator(".device-menu h3").click();
  await expect(page.locator(".device-menu")).toBeVisible();
  await page.locator(".center-panel").click({ position: { x: 40, y: 200 } });
  await expect(page.locator(".device-menu")).toHaveCount(0);
  await devices.click();
  await devices.click();
  await expect(page.locator(".device-menu")).toHaveCount(0);
});

test("romanization replaces non-Latin lyrics in normal, floating and preview views", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const romanization = page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Romanization", { exact: true });
  await romanization.click();
  const samples = [
    ["안녕하세요", "", "annyeonghaseyo"],
    ["Привет", "", "Privet"],
    ["مرحبا", "", "mrHb"],
    ["你好", "", "nǐ-hǎo"],
    // Kana makes the song Japanese, so its kanji read as Japanese too.
    ["東京へ行く", "", "tōkyō e iku"],
    ["東京", "Tokyo", "Tokyo"],
  ];
  const emit = async (text: string, reading: string) => {
    await page.evaluate(
      ({ uri, text, reading }) => {
        (window as any).__clientTest.emit({
          type: "lyrics",
          uri,
          lyrics: {
            source: "Test",
            synced: true,
            instrumental: false,
            lines: [
              {
                text,
                romanization: reading,
                translation: "",
                start_ms: 1000,
                end_ms: 3000,
                words: [{ text, start_ms: 1000, end_ms: 3000 }],
                background: false,
                opposite: false,
              },
            ],
          },
        });
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms: 1500,
          playing: false,
          observed_at: Date.now(),
        });
      },
      { uri: track.uri, text, reading },
    );
  };
  for (const [text, reading, expected] of samples) {
    await emit(text, reading);
    await expect(page.locator(".lyrics-page .lyric-main")).toHaveText(expected);
  }
  await romanization.click();
  await expect(page.locator(".lyrics-page .lyric-main")).toHaveText("東京");
  await romanization.click();
  await openSettings(page, "Effects");
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  for (const [text, reading, expected] of samples) {
    await emit(text, reading);
    // Space Gravity floats each word (and hyphenated part) on its own.
    await expect
      .poll(async () =>
        (await page.locator(".gravity-word:not(.exiting)").allTextContents())
          .join("")
          .replace(/\s/g, ""),
      )
      .toBe(expected.replace(/\s/g, ""));
  }
  await page.locator("footer").getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyric-preview .lyric-main")).toHaveText("Tokyo");
});

test("turning romanization on keeps the lyrics in place", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Test",
        synced: true,
        instrumental: false,
        lines: Array.from({ length: 30 }, (_, index) => ({
          // Romanized Hangul runs about three times as wide, so lines rewrap.
          text: "사랑해요 보고싶어요 안녕하세요",
          romanization: "",
          translation: "",
          start_ms: index * 2000,
          end_ms: index * 2000 + 1900,
          words: [],
          background: false,
          opposite: false,
        })),
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 40500,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  const scroller = page.locator(".lyrics-page .lyric-scroll").first();
  const top = () => scroller.evaluate((node) => node.scrollTop);
  let settled = -1;
  await expect
    .poll(async () => {
      const now = await top();
      const same = now === settled;
      settled = now;
      return same && now > 0;
    })
    .toBe(true);
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Romanization", { exact: true })
    .click();
  await expect(page.locator(".lyrics-page .lyric-main").first()).toContainText(
    "saranghaeyo",
  );
  // Settled by the next frame: no slide to the rewrapped line.
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  const first = await top();
  await page.waitForTimeout(500);
  expect(Math.abs((await top()) - first)).toBeLessThan(2);
});

test("lyrics toolbar toggles romanization, word sync and layout", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const toolbar = page.getByRole("toolbar", { name: "Lyrics" });
  await expect(toolbar.getByTitle("Close lyrics")).toHaveCount(0);
  const saved = (key: string) =>
    page.evaluate(
      (key) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings[key],
      key,
    );
  for (const [title, key] of [
    ["Romanization", "romanized"],
    ["Experimental word sync", "experimental_word_sync"],
  ]) {
    const button = toolbar.getByTitle(title, { exact: true });
    await button.click();
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await expect.poll(() => saved(key)).toBe(true);
  }
  await toolbar.getByTitle("Hide artwork", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toHaveClass(/no-artwork/);
  await expect(page.locator(".lyrics-album")).toBeHidden();
  await toolbar.getByTitle("Hide artwork", { exact: true }).click();
  await toolbar.getByTitle("Artwork on the right", { exact: true }).click();
  // Side-by-side layouts put the artwork after the lyrics.
  await expect(page.locator(".lyrics-page")).toHaveClass(/artwork-right/);
  await toolbar.getByTitle("Compact lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toHaveClass(/narrow/);
  await toolbar.getByTitle("Lyssna settings", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Lyssna settings" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("tab", { name: "Lyrics", exact: true }).click();
  await expect(
    dialog.getByLabel("Lyric text size", { exact: true }),
  ).toHaveValue("36");
  const settingsBox = (await dialog.boundingBox())!;
  expect(settingsBox.width).toBeGreaterThan(760);
  expect(settingsBox.height).toBeGreaterThan(660);
  await page.screenshot({
    path: "target/diagnostics/lyrics-settings-size.png",
    animations: "disabled",
  });
  await expect(
    dialog.getByRole("tab", { name: "Lyrics", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(dialog).toContainText(`v${appVersion}`);
  await expect(
    dialog.getByRole("button", { name: /Website|Discord|Ko-fi/ }),
  ).toHaveCount(0);
  // Search reaches settings on other tabs.
  await dialog.getByLabel("Search settings").fill("gravity");
  await expect(
    dialog.getByRole("switch", { name: "Space Gravity" }),
  ).toBeVisible();
  await dialog.getByRole("switch", { name: "Space Gravity" }).click();
  await expect.poll(() => saved("space_gravity")).toBe(true);
  await dialog.getByLabel("Search settings").fill("nothing like this");
  await expect(dialog).toContainText("No settings match.");
  await dialog.getByLabel("Search settings").fill("");
  await dialog.getByRole("tab", { name: "Sources" }).click();
  await dialog.getByRole("button", { name: "Browse", exact: true }).click();
  await expect(
    page.getByRole("dialog", { name: "TTML Database" }),
  ).toBeVisible();
  await page
    .getByRole("dialog", { name: "TTML Database" })
    .getByRole("button", { name: /close/i })
    .first()
    .click();
  await toolbar.getByTitle("Lyssna settings", { exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  await toolbar.getByTitle("Lyssna settings", { exact: true }).click();
  // About opens from the footer card and lists the credits.
  await expect(dialog.locator(".lyrics-settings-wordmark")).toHaveCount(1);
  await expect(dialog.getByRole("tab", { name: "About" })).toHaveCount(0);
  await dialog.getByRole("button", { name: /About and credits/ }).click();
  await expect(dialog.locator(".credit-person")).toHaveCount(3);
  // Same order as README.md's Credits.
  await expect(dialog.locator(".credit-row").first()).toContainText(
    "Spicy Lyrics",
  );
  await expect(
    dialog.locator(".credit-row", { hasText: "Spotify+" }),
  ).toHaveCount(0);
  await dialog.locator(".credit-row").first().click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "open_link")
            .at(-1)?.args.url,
      ),
    )
    .toBe("https://github.com/iPixelGalaxy/spicy-lyrics");
  await dialog.getByRole("button", { name: /Back to settings/ }).click();
  await expect(dialog.locator(".credit-person")).toHaveCount(0);
  // Credits remain searchable after moving About out of the tab bar.
  await dialog.getByLabel("Search settings").fill("Spikerko");
  await expect(dialog.locator(".credit-person")).toHaveCount(3);
  await dialog.getByLabel("Search settings").fill("");
  await expect(dialog.locator(".credit-person")).toHaveCount(0);
  await dialog.getByRole("tab", { name: "General" }).click();
  await expect(
    dialog.getByRole("combobox", { name: "Audio output" }),
  ).toBeVisible();
  await expect(page.locator(".settings-page")).toHaveCount(0);
  await dialog.getByRole("button", { name: "Close settings" }).click();
  await expect(page.locator(".lyrics-page")).toBeVisible();
});

test("flat paragraph lyrics gain estimated word timing and revert when disabled", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const text = "Hello there. Come back tonight! Stay with me.";
  await page.evaluate(
    ({ uri, text }) => {
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "test",
          synced: false,
          instrumental: false,
          lines: [
            {
              text,
              start_ms: 0,
              end_ms: 180000,
              words: [],
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
            },
          ],
        },
      });
    },
    { uri: track.uri, text },
  );
  await expect(page.locator(".lyrics-page .lyrics-display")).toHaveClass(
    /static-lyrics/,
  );
  const button = page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Experimental word sync", { exact: true });
  await button.click();
  await expect(page.locator(".lyrics-page .lyrics-display")).not.toHaveClass(
    /static-lyrics/,
  );
  await expect(page.locator(".lyrics-page .lyric-main")).toHaveCount(3);
  await expect(page.locator(".lyrics-page .lyric-source")).toContainText(
    "estimated line and word timing",
  );
  await expect(page.locator(".lyrics-page .lyric-word").first()).toBeVisible();
  await button.click();
  await expect(page.locator(".lyrics-page .lyrics-display")).toHaveClass(
    /static-lyrics/,
  );
  await expect(page.locator(".lyrics-page .lyric-main")).toHaveText(text);
});

test("word sync splits merged Apple Music words and says so", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "apple_music",
          synced: true,
          instrumental: false,
          lines: [
            {
              text: "Hey I want you",
              start_ms: 0,
              end_ms: 4000,
              words: [
                { text: "Hey", start_ms: 0, end_ms: 1000 },
                { text: " I want you", start_ms: 1000, end_ms: 4000 },
              ],
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
            },
          ],
        },
      }),
    track.uri,
  );
  const source = page.locator(".lyrics-page .lyric-source");
  await expect(source).toContainText("Apple Music");
  await expect(source).not.toContainText("extra splits");
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Experimental word sync", { exact: true })
    .click();
  await expect(source).toContainText("Apple Music (+ extra splits)");
});

test("pending search debounce cannot reopen search after navigating Home", async ({
  page,
}) => {
  await startClient(page);
  const now = new Date();
  await page.clock.install({ time: now });
  await page.clock.pauseAt(new Date(now.getTime() + 1000));
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("workout");
  await search.press("Enter");
  await page.clock.runFor(50);
  await page.getByTitle("Home", { exact: true }).click();
  await page.clock.runFor(350);
  await expect(page.locator(".personal-home")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1)?.args.key,
      ),
    )
    .toBe("home");
});

test("Spotify search shows public and curated playlist cards with open and play actions", async ({
  page,
}) => {
  await startClient(page);
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("workout");
  await search.press("Enter");
  await page.waitForFunction(() =>
    (window as any).__clientTest.commands.some(
      (entry: any) =>
        entry.command === "browse" && entry.args.key === "search:workout",
    ),
  );
  // The mock's own search results (with no playlists) land 20 ms after the
  // request; send the playlists after them so they are not replaced.
  await expect(page.getByText("Song 1", { exact: true }).first()).toBeVisible();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const request = host.commands
      .filter(
        (entry: any) =>
          entry.command === "browse" && entry.args.key === "search:workout",
      )
      .at(-1).args;
    host.emit({
      type: "search_playlists",
      account: "test-user",
      key: request.key,
      generation: request.generation,
      message: "",
      playlists: [
        {
          id: "0123456789abcdefghijkl",
          name: "Public Workout",
          images: [],
          owner: { id: "listener", display_name: "Playlist creator" },
          collaborative: false,
        },
        {
          id: "1123456789abcdefghijkl",
          name: "Curated Workout",
          images: [],
          owner: { id: "spotify", display_name: "Spotify" },
          collaborative: false,
        },
      ],
    });
  });
  const shelf = page.getByRole("region", { name: "Playlists", exact: true });
  await expect(
    shelf.getByRole("button", { name: "Open Public Workout", exact: true }),
  ).toBeVisible();
  await expect(shelf).toContainText("By Spotify");
  await expect(
    page.getByRole("heading", { name: "Songs", exact: true }),
  ).toBeVisible();
  await shelf
    .getByRole("button", { name: "Play Curated Workout", exact: true })
    .click();
  const playback = await page.evaluate(
    () =>
      (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "player")
        .at(-1).args.request,
  );
  expect(playback).toMatchObject({
    command: "play_home",
    context: "spotify:playlist:1123456789abcdefghijkl",
  });
  await shelf
    .getByRole("button", { name: "Open Public Workout", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1).args.key,
      ),
    )
    .toBe("playlist:0123456789abcdefghijkl");
  await expect(page.locator(".search-playlists")).toHaveCount(0);
});

test("playlist-only search results stay visible when songs are empty or playlists need retry", async ({
  page,
}) => {
  await startClient(page);
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("chill");
  await search.press("Enter");
  await expect(
    page.getByRole("tablist", { name: "Search categories" }),
  ).toBeVisible();
  await page.waitForTimeout(150);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const request = host.commands
      .filter(
        (entry: any) =>
          entry.command === "browse" && entry.args.key === "search:chill",
      )
      .at(-1).args;
    host.emit({
      type: "tracks",
      ...request,
      offset: 0,
      tracks: [],
      more: false,
    });
    host.emit({
      type: "search_playlists",
      account: "test-user",
      key: request.key,
      generation: request.generation,
      message: "Some Spotify-curated playlists may be unavailable.",
      playlists: [
        {
          id: "0123456789abcdefghijkl",
          name: "Chill playlist",
          images: [],
          owner: { id: "listener" },
          collaborative: false,
        },
      ],
    });
  });
  await expect(
    page.getByRole("button", { name: "Open Chill playlist", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "No songs found" }),
  ).toHaveCount(0);
  await expect(page.locator(".search-page")).toContainText(
    "Some Spotify-curated playlists may be unavailable.",
  );
});

test("search finds profiles and loads further pages of a category", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    (window as any).__clientTest.userProfiles = {
      friend: {
        id: "friend",
        name: "Friend Person",
        images: [],
        public_playlists: 0,
        followers: 2,
        following: 1,
        is_current_user: false,
        is_following: false,
        allow_follows: true,
        sections: [],
      },
    };
  });
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("friend");
  await search.press("Enter");
  const results = page.getByRole("region", {
    name: "Search results for friend",
    exact: true,
  });
  await results.getByRole("tab", { name: "Songs", exact: true }).click();
  await expect(results.getByText("Song 20", { exact: true })).toBeVisible();
  await results
    .getByRole("button", { name: "Load more songs", exact: true })
    .click();
  await expect(results.getByText("Song 25", { exact: true })).toBeVisible();
  await expect(
    results.getByRole("button", { name: "Load more songs", exact: true }),
  ).toHaveCount(0);
  const request = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "search_page")
      .at(-1),
  );
  expect(request.args).toEqual({ query: "friend", kind: "tracks", offset: 20 });
  await results.getByRole("tab", { name: "Profiles", exact: true }).click();
  const card = results.locator(".home-card.artist-card");
  await expect(card).toContainText("Friend Person");
  await card
    .getByRole("button", { name: "Friend Person", exact: true })
    .click();
  await expect(page.locator(".profile-hero h1")).toHaveText("Friend Person");
});

test("search matches compact Spotify layout and music category filters stay functional", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("arianna");
  await search.press("Enter");
  const results = page.getByRole("region", {
    name: "Search results for arianna",
    exact: true,
  });
  await expect(results.locator(".search-top-result")).toBeVisible();
  await expect(results.getByRole("tab")).toHaveText([
    "All",
    "Songs",
    "Artists",
    "Playlists",
    "Albums",
    "Profiles",
  ]);
  await expect(page.locator(".page-hero")).toHaveCount(0);
  expect(
    await results
      .locator(".search-top-result")
      .evaluate((element) => element.getBoundingClientRect().height),
  ).toBeLessThan(110);
  await page.screenshot({ path: "test-results/search-wide.png" });
  await results.getByRole("tab", { name: "Songs", exact: true }).click();
  await expect(results.locator(".search-result-row")).toHaveCount(20);
  await expect(results.locator(".home-shelf")).toHaveCount(0);
  await results
    .getByRole("button", { name: "Save Song 1 to Liked Songs", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_item")
            .at(-1)?.args,
      ),
    )
    .toMatchObject({
      uri: "spotify:track:0000000000000000000001",
      saved: true,
    });
  await results
    .locator(".search-result-row")
    .first()
    .click({ button: "right" });
  await expect(page.getByRole("menu", { name: "Song actions" })).toBeVisible();
  await page.keyboard.press("Escape");
  await results.getByRole("tab", { name: "Artists", exact: true }).click();
  await expect(
    results.getByRole("region", { name: "Artists", exact: true }),
  ).toBeVisible();
  await expect(results.locator(".home-shelf-items")).toHaveClass(/expanded/);
  await results.getByRole("tab", { name: "Playlists", exact: true }).click();
  await expect(
    results.getByRole("heading", { name: "No playlists found", exact: true }),
  ).toBeVisible();
  await results.getByRole("tab", { name: "Albums", exact: true }).click();
  await expect(
    results.getByRole("region", { name: "Albums", exact: true }),
  ).toBeVisible();
  await results
    .getByRole("button", { name: "Open Test album", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText(
    "Test album",
  );
});

test("compact search fits narrow panels, supports keyboard filters and resets for another query", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1100, height: 800 });
  await startClient(page);
  const search = page.getByRole("textbox", {
    name: "Search Spotify",
    exact: true,
  });
  await search.fill("first");
  await search.press("Enter");
  await expect(page.locator(".search-top-result")).toBeVisible();
  const all = page.getByRole("tab", { name: "All", exact: true });
  await all.focus();
  await page.keyboard.press("ArrowRight");
  await expect(
    page.getByRole("tab", { name: "Songs", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  const fits = await page.locator(".search-page").evaluate((element) => {
    const box = element.getBoundingClientRect();
    return Array.from(element.querySelectorAll(".search-result-row")).every(
      (row) => row.getBoundingClientRect().right <= box.right + 1,
    );
  });
  expect(fits).toBe(true);
  await page.screenshot({ path: "test-results/search-narrow.png" });
  await search.fill("second");
  await search.press("Enter");
  await expect(
    page.getByRole("tab", { name: "All", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("region", {
      name: "Search results for second",
      exact: true,
    }),
  ).toBeVisible();
});

test("custom lyric fonts keep italic glyph overhang inside gradient paint bounds", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate(
    ({ uri, settings }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "settings",
        settings: {
          ...settings,
          lyric_font: "Arial",
          lyric_size: 72,
          reduce_motion: true,
        },
      });
      host.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "test",
          synced: true,
          instrumental: false,
          lines: [
            {
              text: "f",
              start_ms: 1000,
              end_ms: 3000,
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [{ text: "f", start_ms: 1000, end_ms: 3000 }],
            },
            {
              text: "jiffy",
              start_ms: 4000,
              end_ms: 4500,
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [{ text: "jiffy", start_ms: 4000, end_ms: 4500 }],
            },
          ],
        },
      });
      host.emit({
        type: "native_position",
        uri,
        position_ms: 1500,
        playing: false,
        observed_at: Date.now(),
      });
    },
    { uri: track.uri, settings: emptyState.settings },
  );
  await page.addStyleTag({ content: ".lyric-line { font-style: italic; }" });
  await expect(page.locator(".lyrics-page .lyric-letter")).toHaveCount(1);
  const ink = await page
    .locator(
      ".lyrics-page .lyric-letter, .lyrics-page .lyric-syllable:not(.emphasis)",
    )
    .evaluateAll((elements) => {
      const context = document.createElement("canvas").getContext("2d")!;
      return elements.map((element) => {
        const style = getComputedStyle(
          element.querySelector(".lyric-paint") ?? element,
        );
        context.font = `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        const metrics = context.measureText(element.textContent ?? "");
        return {
          left: Math.max(0, metrics.actualBoundingBoxLeft),
          right: Math.max(0, metrics.actualBoundingBoxRight - metrics.width),
          paintLeft: parseFloat(style.paddingLeft),
          paintRight: parseFloat(style.paddingRight),
          background: style.backgroundImage,
          width: element.getBoundingClientRect().width,
          advance: metrics.width,
        };
      });
    });
  await page.screenshot({ path: "test-results/lyric-font-overhang.png" });
  expect(ink.some((item) => item.left > 0.5 || item.right > 0.5)).toBe(true);
  for (const item of ink) {
    expect(item.paintLeft + 0.5).toBeGreaterThanOrEqual(item.left);
    expect(item.paintRight + 0.5).toBeGreaterThanOrEqual(item.right);
    expect(item.background).toContain("linear-gradient");
    expect(Math.abs(item.width - item.advance)).toBeLessThan(1);
  }
});

test("curated playlist descriptions display link labels and open Spotify playlists", async ({
  page,
}) => {
  await startClient(page, "home");
  await page
    .getByRole("button", { name: "Open Daily Mix 1", exact: true })
    .click();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const request = host.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1);
    host.emit({
      type: "browse_info",
      generation: request.args.generation,
      info: {
        key: request.args.key,
        title: "Curated test",
        albums: [],
        images: [],
        subtitle:
          'Here\'s some <a href="spotify:playlist:1123456789abcdefghijkl">happy</a>, <a href="https://open.spotify.com/playlist/2123456789abcdefghijkl?si=test">dystopian</a> &amp; gentle music.',
        playlist: { owner: "spotify", format: "", total_tracks: 50 },
      },
    });
  });
  const description = page.locator(".hero-description");
  await expect(description).toHaveText(
    "Here's some happy, dystopian & gentle music.",
  );
  await description.getByRole("link", { name: "happy", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1)?.args.key,
      ),
    )
    .toBe("playlist:1123456789abcdefghijkl");
});

test("playlist descriptions preserve formatting without executing or loading provider markup", async ({
  page,
}) => {
  const resourceRequests: string[] = [];
  await page.route("https://markup.example.test/**", async (route) => {
    resourceRequests.push(route.request().url());
    await route.fulfill({ status: 200, body: "" });
  });
  await startClient(page, "home");
  await page
    .getByRole("button", { name: "Open Daily Mix 1", exact: true })
    .click();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const request = host.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1);
    host.emit({
      type: "browse_info",
      generation: request.args.generation,
      info: {
        key: request.args.key,
        title: "Curated test",
        albums: [],
        images: [],
        subtitle:
          '<strong>Safe</strong><br>Music &#x266b; <a href="javascript:window.__descriptionExecuted=1">blocked</a> <a href="https://open.spotify.com.evil.test/playlist/1123456789abcdefghijkl">spoof</a><script>window.__descriptionExecuted=1</script><img src="https://markup.example.test/image" onerror="window.__descriptionExecuted=1"><iframe src="https://markup.example.test/frame"></iframe><style>body { display:none; }</style>',
        playlist: { owner: "spotify", format: "", total_tracks: 50 },
      },
    });
  });
  const description = page.locator(".hero-description");
  await expect(description.locator("strong")).toHaveText("Safe");
  await expect(description.locator("br")).toHaveCount(1);
  await expect(description).toContainText("Music ♫ blocked spoof");
  await expect(description.locator("script,img,iframe,style")).toHaveCount(0);
  await expect(description.getByRole("link")).toHaveCount(0);
  expect(
    await page.evaluate(() => (window as any).__descriptionExecuted),
  ).toBeUndefined();
  expect(resourceRequests).toEqual([]);
});

test("Space after clicking a player button only plays or pauses", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...base, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  const player = () =>
    page.evaluate(() =>
      (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "player")
        .map((entry: any) => entry.args.request.command),
    );
  await page.getByTitle("Shuffle", { exact: true }).click();
  await page.getByTitle("Next song", { exact: true }).click();
  await expect.poll(player).toEqual(["shuffle", "next"]);
  await page.keyboard.press("Space");
  await page.keyboard.press("Enter");
  await expect.poll(player).toEqual(["shuffle", "next", "pause"]);
});

test("owners edit playlist name and description", async ({ page }) => {
  await startClient(page);
  const id = "4123456789abcdefghijkl";
  await page.evaluate(
    (id) =>
      (window as any).__clientTest.emit({
        type: "library",
        playlists: [
          {
            id,
            name: "Road trip",
            images: [],
            owner: { id: "test-user" },
            collaborative: false,
          },
        ],
      }),
    id,
  );
  await page.locator(".library-item", { hasText: "Road trip" }).first().click();
  const rows = page.locator(".track-row");
  await expect(rows).toHaveCount(150);
  const last = (command: string) =>
    page.evaluate(
      (command) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === command)
          .at(-1)?.args,
      command,
    );

  await page.locator(".editable-title").click();
  const dialog = page.locator("form.edit-playlist");
  await expect(dialog.getByLabel("Name")).toHaveValue("Road trip");
  await dialog.getByLabel("Name").fill("Night drive");
  await dialog.getByLabel("Description").fill("Late roads");
  // A picked image is cropped to a square JPEG and previewed before saving.
  await dialog.getByLabel("Choose playlist cover").setInputFiles({
    name: "cover.png",
    mimeType: "image/png",
    buffer: Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
      "base64",
    ),
  });
  await expect(dialog.locator(".edit-playlist-cover img")).toHaveAttribute(
    "src",
    /^data:image\/jpeg;base64,/,
  );
  await expect(dialog).toContainText("you have the right to use this image");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toHaveCount(0);
  const saved = await last("edit_playlist");
  expect(saved).toMatchObject({
    name: "Night drive",
    description: "Late roads",
    playlist: { id },
  });
  // JPEG data starts with FF D8 FF, "/9j/" in base64.
  expect(saved.cover).toMatch(/^\/9j\//);
  // The saved cover shows at once, and the cover itself reopens the editor.
  const heroCover = page.getByRole("button", {
    name: "Choose photo",
    exact: true,
  });
  await expect(heroCover.locator("img")).toHaveAttribute(
    "src",
    /^data:image\/jpeg;base64,/,
  );
  await heroCover.click();
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();
});

test("songs drag into place in an editable playlist", async ({ page }) => {
  await startClient(page);
  const id = "4123456789abcdefghijkl";
  await page.evaluate(
    (id) =>
      (window as any).__clientTest.emit({
        type: "library",
        playlists: [
          {
            id,
            name: "Road trip",
            images: [],
            owner: { id: "test-user" },
            collaborative: false,
          },
        ],
      }),
    id,
  );
  await page.locator(".library-item", { hasText: "Road trip" }).first().click();
  const rows = page.locator(".track-row");
  await expect(rows).toHaveCount(150);
  const last = (command: string) =>
    page.evaluate(
      (command) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === command)
          .at(-1)?.args,
      command,
    );

  // Drop on the lower half of the third row: the first song lands third.
  const third = await rows.nth(2).boundingBox();
  await rows.nth(0).dragTo(rows.nth(2), {
    targetPosition: { x: 40, y: third!.height - 4 },
  });
  await expect
    .poll(() => last("move_playlist_song"))
    .toMatchObject({
      from: 0,
      to: 2,
      uri: "spotify:track:item0",
    });
  await expect(rows.nth(0)).toContainText("Song 2");
  await expect(rows.nth(2)).toContainText("Song 1");
});

test("Spotify folders group the sidebar and open in place", async ({
  page,
}) => {
  await startClient(page);
  const road = { id: "road", name: "Road & Rain" };
  const night = { id: "night", name: "Night" };
  const playlist = (index: number, folders: object[] = []) => ({
    id: `${index}123456789abcdefghijkl`.slice(0, 22),
    name: `List ${index}`,
    images: [],
    owner: { id: "test-user" },
    collaborative: false,
    folders,
  });
  await page.evaluate(
    (playlists) =>
      (window as any).__clientTest.emit({ type: "library", playlists }),
    [playlist(1, [road]), playlist(2, [road, night]), playlist(3)],
  );
  const sidebar = page.locator(".library-list");
  const folder = sidebar.locator(".spotify-folder").first();
  await expect(folder).toContainText("Road & Rain");
  await expect(folder).toContainText("Folder · 2 playlists");
  await expect(sidebar.getByText("List 1")).toBeHidden();
  // Like Spotify, a folder opens as its own list with a back button.
  await folder.click();
  const header = sidebar.locator(".library-folder-header");
  await expect(header).toContainText("Road & Rain");
  await expect(sidebar.getByText("List 1")).toBeVisible();
  await expect(sidebar.getByText("List 3")).toBeHidden();
  await expect(sidebar.getByText("Liked Songs")).toBeHidden();
  const inner = sidebar.locator(".spotify-folder");
  await expect(inner).toContainText("Night");
  await inner.click();
  await expect(header).toContainText("Night");
  await expect(sidebar.getByText("List 2")).toBeVisible();
  await expect(sidebar.getByText("List 1")).toBeHidden();
  await sidebar.getByText("List 2").click();
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("List 2");
  await header.getByRole("button", { name: "Back" }).click();
  await expect(header).toContainText("Road & Rain");
  await header.getByRole("button", { name: "Back" }).click();
  await expect(header).toHaveCount(0);
  await expect(sidebar.getByText("List 3")).toBeVisible();
  // Searching the library lists matching playlists without folders.
  await page.getByLabel("Search library").fill("List 2");
  await expect(sidebar.locator(".spotify-folder")).toHaveCount(0);
  await expect(sidebar.getByText("List 2")).toBeVisible();
});

test("Create folder adds a Spotify folder that shows while empty", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Create folder", { exact: true }).click();
  await page.getByPlaceholder("Folder name").fill("Workouts");
  await page
    .locator(".dialog")
    .getByRole("button", { name: "Create folder", exact: true })
    .click();
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "create_folder")
          .at(-1)?.args,
    ),
  ).toEqual({ name: "Workouts" });
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "library_folders",
      folders: [{ id: "workouts", name: "Workouts", parents: [] }],
    }),
  );
  const folder = page.locator(".library-list .spotify-folder");
  await expect(folder).toContainText("Folder · 0 playlists");
});

test("folders rename from their right-click menu", async ({ page }) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "library_folders",
      folders: [{ id: "workouts", name: "Workouts", parents: [] }],
    }),
  );
  await page
    .locator(".library-list .spotify-folder")
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Rename" }).click();
  const dialog = page.locator("form.edit-playlist");
  await expect(dialog.getByLabel("Name")).toHaveValue("Workouts");
  await dialog.getByLabel("Name").fill("Gym");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "rename_folder")
          .at(-1)?.args,
    ),
  ).toEqual({ id: "workouts", name: "Gym" });
});

test("playlists move into and out of folders from their menu", async ({
  page,
}) => {
  await startClient(page);
  const id = "5123456789abcdefghijkl";
  await page.evaluate((id) => {
    const test = (window as any).__clientTest;
    test.emit({
      type: "library",
      playlists: [
        {
          id,
          name: "Road trip",
          images: [],
          owner: { id: "test-user" },
          collaborative: false,
        },
      ],
    });
    test.emit({
      type: "library_folders",
      folders: [
        { id: "gym", name: "Gym", parents: [] },
        { id: "legs", name: "Legs", parents: ["gym"] },
      ],
    });
  }, id);
  const last = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "move_to_folder")
          .at(-1)?.args,
    );
  await page
    .locator(".library-row", { hasText: "Road trip" })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to folder" }).click();
  const picker = page.getByRole("menu", { name: "Move to folder" });
  await expect(picker.getByRole("menuitem")).toHaveText(["Gym", "Legs"]);
  await page.keyboard.press("Escape");
  await expect(picker).toHaveCount(0);
  expect(await last()).toBeUndefined();
  await page
    .locator(".library-row", { hasText: "Road trip" })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to folder" }).click();
  await picker.getByRole("menuitem", { name: "Legs" }).click();
  expect(await last()).toEqual({ playlist: id, folder: "legs" });

  // Once inside a folder, the picker also offers to take it out.
  await page.evaluate(
    (id) =>
      (window as any).__clientTest.emit({
        type: "library",
        playlists: [
          {
            id,
            name: "Road trip",
            images: [],
            owner: { id: "test-user" },
            collaborative: false,
            folders: [
              { id: "gym", name: "Gym" },
              { id: "legs", name: "Legs" },
            ],
          },
        ],
      }),
    id,
  );
  await page.locator(".spotify-folder").first().click();
  await page.locator(".spotify-folder").first().click();
  await page
    .locator(".library-row", { hasText: "Road trip" })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to folder" }).click();
  await expect(picker.getByRole("menuitem", { name: "Legs" })).toBeDisabled();
  await picker.getByRole("menuitem", { name: "Remove from folder" }).click();
  expect(await last()).toEqual({ playlist: id, folder: null });
});

test("folder picker follows library updates and closes on sign-out", async ({
  page,
}) => {
  await startClient(page);
  const playlist = {
    id: "5123456789abcdefghijkl",
    name: "Shared road trip",
    images: [],
    owner: { id: "friend" },
    collaborative: false,
  };
  await page.evaluate((playlist) => {
    const emit = (window as any).__clientTest.emit;
    emit({ type: "library", playlists: [playlist] });
    emit({
      type: "library_folders",
      folders: [{ id: "gym", name: "Gym", parents: [] }],
    });
  }, playlist);
  await page
    .locator(".library-row", { hasText: playlist.name })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move to folder" }).click();
  const picker = page.getByRole("menu", { name: "Move to folder" });
  await expect(picker.getByRole("menuitem", { name: "Gym" })).toBeEnabled();
  await expect(
    picker.getByRole("menuitem", { name: "Remove from folder" }),
  ).toHaveCount(0);
  await page.evaluate((playlist) => {
    (window as any).__clientTest.emit({
      type: "library",
      playlists: [{ ...playlist, folders: [{ id: "gym", name: "Gym" }] }],
    });
  }, playlist);
  await expect(picker.getByRole("menuitem", { name: "Gym" })).toBeDisabled();
  await expect(
    picker.getByRole("menuitem", { name: "Remove from folder" }),
  ).toBeVisible();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  await expect(picker).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText(
    "Spotify Premium required",
  );
  await page.evaluate((playlist) => {
    const emit = (window as any).__clientTest.emit;
    emit({ type: "session", profile: { id: "another-user" } });
    emit({ type: "library", playlists: [playlist] });
    emit({
      type: "library_folders",
      folders: [{ id: "gym", name: "Gym", parents: [] }],
    });
  }, playlist);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(picker).toHaveCount(0);
  await expect(
    page.locator(".library-row", { hasText: playlist.name }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.filter(
        (entry: any) => entry.command === "move_to_folder",
      ),
    ),
  ).toEqual([]);
});

async function startFolderDeletionClient(page: Page) {
  await startClient(page);
  const folders = [
    { id: "road", name: "Road", parents: [] as string[] },
    { id: "night", name: "Night", parents: ["road"] },
  ];
  const playlists = [
    {
      id: "5123456789abcdefghijkl",
      name: "Road trip",
      images: [],
      owner: { id: "test-user" },
      collaborative: false,
      folders: [{ id: "road", name: "Road" }],
    },
    {
      id: "6123456789abcdefghijkl",
      name: "Shared nights",
      images: [],
      owner: { id: "friend" },
      collaborative: true,
      folders: [
        { id: "road", name: "Road" },
        { id: "night", name: "Night" },
      ],
    },
  ];
  await page.evaluate(
    ({ playlists, folders }) => {
      const emit = (window as any).__clientTest.emit;
      emit({ type: "library", playlists });
      emit({ type: "library_folders", folders });
    },
    { playlists, folders },
  );
  return { playlists, folders };
}

test("Liquid Glass combines shading without changing backdrop pixels", async ({
  page,
}) => {
  await startClient(page, "home");
  await page.evaluate(
    ({ settings, playback, track }) => {
      const emit = (window as any).__clientTest.emit;
      emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: false },
        observed_at: Date.now(),
        changed: false,
      });
      emit({ type: "settings", settings: { ...settings, liquid_glass: true } });
    },
    { settings: emptyState.settings, playback: emptyState.playback, track },
  );
  const backdrop = page.locator(".glass-backdrop");
  await expect(backdrop.locator(".ambient-layer")).toBeVisible();
  await expect(backdrop.locator(".ambient-shade")).toHaveCount(0);
  await expect(page.locator(".now-playing .ambient-shade")).toHaveCount(1);
  // Finish the existing 500 ms artwork transition before comparing paint.
  await page.waitForTimeout(700);
  const mask = [page.locator(".marquee")];
  const optimized = await backdrop.screenshot({ animations: "disabled", mask });
  await backdrop.locator(".ambience").evaluate((element) => {
    const shade = document.createElement("div");
    shade.className = "ambient-shade";
    element.append(shade);
  });
  await page.addStyleTag({
    content:
      ".glass-backdrop-shade { background: radial-gradient(ellipse at 50% 0%, transparent, #00000059 70%), linear-gradient(#0000002e, #00000066); }",
  });
  const original = await backdrop.screenshot({ animations: "disabled", mask });
  const difference = await page.evaluate(
    async ({ original, optimized }) => {
      const canvas = document.createElement("canvas");
      const context = canvas.getContext("2d", { willReadFrequently: true })!;
      const pixels = async (base64: string) => {
        const image = new Image();
        image.src = `data:image/png;base64,${base64}`;
        await image.decode();
        canvas.width = image.width;
        canvas.height = image.height;
        context.drawImage(image, 0, 0);
        return context.getImageData(0, 0, image.width, image.height).data;
      };
      const before = await pixels(original),
        after = await pixels(optimized);
      let changed = 0;
      for (let index = 0; index < before.length; index += 4) {
        if (
          [0, 1, 2].some(
            (channel) =>
              Math.abs(before[index + channel] - after[index + channel]) > 2,
          )
        )
          changed++;
      }
      return changed / (before.length / 4);
    },
    {
      original: original.toString("base64"),
      optimized: optimized.toString("base64"),
    },
  );
  expect(difference).toBeLessThan(0.001);
});

test("playlist artwork loads near the viewport and still loads after scrolling", async ({
  page,
}) => {
  await startClient(page);
  await expect(page.locator(".track-row")).toHaveCount(150);
  const requests = new Set<number>();
  await page.route("https://artwork.example.test/memory/**", (route) => {
    const index = Number(
      new URL(route.request().url()).pathname.split("/").at(-1)?.split(".")[0],
    );
    requests.add(index);
    return route.fulfill({
      contentType: "image/svg+xml",
      body: '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="640"><rect width="640" height="640" fill="#9670a7"/></svg>',
    });
  });
  const count = 300;
  await page.evaluate(
    ({ track, count }) => {
      const host = (window as any).__clientTest;
      const browse = host.commands
        .filter((entry: any) => entry.command === "browse")
        .at(-1).args;
      host.emit({
        type: "tracks",
        key: browse.key,
        generation: browse.generation,
        offset: 0,
        more: false,
        tracks: Array.from({ length: count }, (_, index) => ({
          ...track,
          id: `memory${index}`,
          uri: `spotify:track:memory${index}`,
          name: `Memory song ${index + 1}`,
          album: {
            ...track.album,
            images: [
              { url: `https://artwork.example.test/memory/${index}.svg` },
            ],
          },
        })),
      });
    },
    { track, count },
  );
  const rows = page.locator(".track-row");
  // Long lists render a window of rows around the viewport.
  await expect(rows.first()).toContainText("Memory song 1");
  await expect
    .poll(() =>
      rows
        .first()
        .locator("img")
        .evaluate(
          (image) =>
            image instanceof HTMLImageElement &&
            image.complete &&
            image.naturalWidth > 0,
        ),
    )
    .toBe(true);
  expect(requests.has(count - 1)).toBe(false);
  expect(requests.size).toBeLessThan(count / 3);
  await page.locator(".browse-scroll").evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  const last = page.locator(".track-row", {
    hasText: `Memory song ${count}`,
  });
  await expect(last).toBeVisible();
  await expect
    .poll(() =>
      last
        .locator("img")
        .evaluate(
          (image) =>
            image instanceof HTMLImageElement &&
            image.complete &&
            image.naturalWidth > 0,
        ),
    )
    .toBe(true);
  expect(requests.has(count - 1)).toBe(true);
  await expect(last.locator(".track-number > span")).toHaveText(String(count));
});

test("lyric clock frames reuse click handlers and keep seeking functional", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate(
    ({ track, playback }) => {
      const { emit } = (window as any).__clientTest;
      emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: true,
          progress_ms: 1000,
        },
        observed_at: Date.now(),
        changed: false,
      });
      emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: Array.from({ length: 20 }, (_, index) => ({
            start_ms: index * 20_000,
            end_ms: (index + 1) * 20_000,
            text: `Line ${index + 1}`,
            words: [
              {
                start_ms: index * 20_000,
                end_ms: (index + 1) * 20_000,
                text: `Line ${index + 1}`,
              },
            ],
          })),
        },
      });
    },
    { track, playback: emptyState.playback },
  );
  const lines = page.locator(".lyrics-page .lyric-line");
  await expect(lines).toHaveCount(20);
  const assignments = await page.evaluate(
    () =>
      new Promise<number>((resolve) => {
        const descriptor = Object.getOwnPropertyDescriptor(
          HTMLElement.prototype,
          "onclick",
        )!;
        let count = 0,
          frames = 0;
        Object.defineProperty(HTMLElement.prototype, "onclick", {
          ...descriptor,
          set(value) {
            if (this.closest(".lyrics-page")) count++;
            descriptor.set!.call(this, value);
          },
        });
        const tick = () => {
          if (++frames < 15) requestAnimationFrame(tick);
          else {
            Object.defineProperty(HTMLElement.prototype, "onclick", descriptor);
            resolve(count);
          }
        };
        requestAnimationFrame(tick);
      }),
  );
  expect(assignments).toBeLessThan(40);
  await lines.nth(1).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request,
      ),
    )
    .toMatchObject({ command: "seek", positionMs: 19_700 });
  // Without Seek fade-in compensation, a click seeks to the line itself.
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Seek fade-in compensation" }).click();
  await page.keyboard.press("Escape");
  await lines.nth(2).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request,
      ),
    )
    .toMatchObject({ command: "seek", positionMs: 40_000 });
});

async function openFolderDeletion(page: Page) {
  await page
    .locator(".library-list .spotify-folder")
    .filter({ hasText: "Road" })
    .click({ button: "right" });
  await page
    .getByRole("menuitem", { name: "Delete folder", exact: true })
    .click();
  return page.getByRole("alertdialog", { name: "Delete folder?" });
}

test("folder deletion confirms, cancels and preserves playlists through refresh", async ({
  page,
}) => {
  const { playlists } = await startFolderDeletionClient(page);
  const dialog = await openFolderDeletion(page);
  await expect(dialog).toContainText(
    'Delete "Road" from your Spotify library?',
  );
  await expect(dialog).toContainText(
    "Playlists and subfolders inside will move up one level",
  );
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(
    dialog.getByRole("button", { name: "Delete folder" }),
  ).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  await openFolderDeletion(page);
  await page.locator(".modal-backdrop").click({ position: { x: 2, y: 2 } });
  await expect(dialog).toHaveCount(0);
  await openFolderDeletion(page);
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);
  const commands = () =>
    page.evaluate(() =>
      (window as any).__clientTest.commands.filter(
        (entry: any) => entry.command === "delete_folder",
      ),
    );
  expect(await commands()).toEqual([]);

  await openFolderDeletion(page);
  await dialog.getByRole("button", { name: "Delete folder" }).click();
  await expect(dialog).toHaveCount(0);
  expect(await commands()).toEqual([
    { command: "delete_folder", args: { id: "road" } },
  ]);
  // Keep the current tree until the backend has confirmed and refreshed it.
  const summaries = page.locator(".library-list .spotify-folder");
  await expect(summaries.filter({ hasText: "Road" })).toContainText(
    "2 playlists",
  );
  await page.evaluate((playlists) => {
    const emit = (window as any).__clientTest.emit;
    emit({
      type: "library",
      playlists: playlists.map((playlist) => ({
        ...playlist,
        folders: playlist.folders.slice(1),
      })),
    });
    emit({
      type: "library_folders",
      folders: [{ id: "night", name: "Night", parents: [] }],
    });
  }, playlists);
  await expect(summaries.filter({ hasText: "Road" })).toHaveCount(0);
  await expect(
    page.locator(".library-row", { hasText: "Road trip" }),
  ).toBeVisible();
  await summaries.filter({ hasText: "Night" }).click();
  await expect(
    page.locator(".library-row", { hasText: "Shared nights" }),
  ).toBeVisible();
});

test("failed folder deletion reports the backend error and keeps the library", async ({
  page,
}) => {
  await startFolderDeletionClient(page);
  const dialog = await openFolderDeletion(page);
  await dialog.getByRole("button", { name: "Delete folder" }).click();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "notice",
      error: true,
      message: "Spotify did not confirm the folder removal",
    }),
  );
  await expect(page.locator(".toast.error")).toContainText(
    "Spotify did not confirm the folder removal",
  );
  await expect(
    page.locator(".library-list .spotify-folder").filter({ hasText: "Road" }),
  ).toContainText("2 playlists");
  // The folder still holds its playlists.
  await page
    .locator(".library-list .spotify-folder")
    .filter({ hasText: "Road" })
    .click();
  await expect(
    page.locator(".library-row", { hasText: "Road trip" }),
  ).toHaveCount(1);
  await page
    .locator(".library-list .spotify-folder")
    .filter({ hasText: "Night" })
    .click();
  await expect(
    page.locator(".library-row", { hasText: "Shared nights" }),
  ).toHaveCount(1);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.filter((entry: any) =>
        ["remove_playlist", "remove_from_playlist"].includes(entry.command),
      ),
    ),
  ).toEqual([]);
});

test("folder deletion closes on sign-out and cannot carry into another account", async ({
  page,
}) => {
  const library = await startFolderDeletionClient(page);
  const dialog = await openFolderDeletion(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "signed_out" }),
  );
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText(
    "Spotify Premium required",
  );
  await page.evaluate(({ playlists, folders }) => {
    const emit = (window as any).__clientTest.emit;
    emit({ type: "session", profile: { id: "another-user" } });
    emit({ type: "library", playlists });
    emit({ type: "library_folders", folders });
  }, library);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(dialog).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.filter(
        (entry: any) => entry.command === "delete_folder",
      ),
    ),
  ).toEqual([]);
});

test("artist You Liked counts liked songs and opens Liked Songs by the artist", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  const pick = {
    uri: "spotify:album:0123456789abcdefghij09",
    route: "album:0123456789abcdefghij09",
    name: "Picked album",
    subtitle: "Album • New Release",
    images: track.album.images,
    kind: "album",
    playable: true,
    format: "",
  };
  const showArtist = async (withPick: boolean) =>
    page.evaluate(
      ({ key, track, pick }) => {
        const host = (window as any).__clientTest;
        const generation = host.commands
          .filter(
            (entry: any) =>
              entry.command === "browse" && entry.args.key === key,
          )
          .at(-1).args.generation;
        host.emit({
          type: "browse_info",
          generation,
          info: {
            key,
            title: "Test artist",
            subtitle: "Artist",
            images: track.album.images,
            artist: { ...track.artists[0], images: track.album.images },
            albums: [],
            overview: {
              popular: [{ track, streams: 1 }],
              pick,
              source_url: "",
              fetched_at: 0,
              details_loaded: true,
            },
          },
        });
      },
      {
        key: `artist:${track.artists[0].id}`,
        track,
        pick: withPick ? pick : undefined,
      },
    );
  // No liked songs and no pick: nothing beside Popular.
  await page.locator(".player-bar .artist-link").first().click();
  await showArtist(false);
  await expect(page.locator(".artist-top section")).toBeVisible();
  await expect(page.locator(".artist-liked")).toHaveCount(0);
  // A pick alone shows without You Liked.
  await showArtist(true);
  await expect(page.locator(".artist-liked h2")).toHaveText(["Artist pick"]);
  await expect(page.locator(".artist-pick")).toContainText(
    "Posted By Test artist",
  );
  await expect(page.locator(".artist-pick")).toContainText("Picked album");
  // Liked songs put You Liked first, the pick below.
  await page.evaluate(() => ((window as any).__clientTest.artistLiked = 3));
  await page.locator(".home-button").click();
  await page.locator(".player-bar .artist-link").first().click();
  await showArtist(true);
  await expect(page.locator(".artist-liked h2")).toHaveText([
    "You Liked",
    "Artist pick",
  ]);
  await expect(page.locator(".artist-liked-row small")).toHaveText(
    "By Test artist",
  );
  await page.getByRole("button", { name: "3 songs" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1)?.args.key,
      ),
    )
    .toBe(`liked-artist:${track.artists[0].id}`);
  await expect(page.locator(".hero-copy h1")).toHaveText(
    "Liked Songs by Test artist",
  );
});
test("artist pages show Spotify's shelves, About card and sticky bar", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.locator(".player-bar .artist-link").first().click();
  const key = `artist:${track.artists[0].id}`;
  const shelfItem = (index: number, kind: string) => ({
    uri: `spotify:${kind}:${String(index).padStart(22, "0")}`,
    route: `${kind}:${String(index).padStart(22, "0")}`,
    name: `${kind} ${index}`,
    subtitle: kind === "artist" ? "Artist" : "2010 • Album",
    images: track.album.images,
    kind,
    playable: kind !== "artist",
    format: "",
  });
  const section = (title: string, kind: string) => ({
    uri: `artist-section:${title}`,
    title,
    subtitle: "",
    items: [1, 2, 3].map((index) => shelfItem(index, kind)),
  });
  await page.evaluate(
    ({ key, track, sections }) => {
      const host = (window as any).__clientTest;
      const generation = host.commands
        .filter(
          (entry: any) => entry.command === "browse" && entry.args.key === key,
        )
        .at(-1).args.generation;
      host.emit({
        type: "browse_info",
        generation,
        info: {
          key,
          title: "Test artist",
          subtitle: "Artist",
          images: track.album.images,
          artist: { ...track.artists[0], images: track.album.images },
          albums: [],
          overview: {
            header_image: track.album.images[0].url,
            verified: true,
            monthly_listeners: 72_847_177,
            followers: 112_804_519,
            world_rank: 21,
            gallery_image: track.album.images[0].url,
            biography: "A long biography. ".repeat(80),
            biography_html:
              'Thanks to <a href="spotify:artist:0123456789abcdefghij07">Mentor</a>, a long story.',
            gallery: [
              "https://artwork.example.test/artist/one.svg",
              "https://artwork.example.test/artist/two.svg",
            ],
            top_cities: [
              { city: "London", country: "GB", listeners: 1_076_135 },
            ],
            popular: [{ track, streams: 3_271_340_476 }],
            sections,
            source_url: "",
            fetched_at: 0,
            details_loaded: true,
          },
        },
      });
    },
    {
      key,
      track,
      sections: [
        section("Featuring Test artist", "playlist"),
        section("Fans also like", "artist"),
        section("Appears On", "album"),
      ],
    },
  );
  const sticky = page.locator(".artist-sticky");
  await expect(sticky).not.toHaveClass(/shown/);
  // Spotify's order: Featuring, then About, then the other shelves.
  const headings = page.locator(
    ".artist-shelf .home-shelf-heading h2, .artist-about > h2",
  );
  await expect(headings).toHaveText([
    "Featuring Test artist",
    "About",
    "Fans also like",
    "Appears On",
  ]);
  // Shelves keep the page's left gutter, aligned with About.
  const aboutLeft = (await page.locator(".artist-about > h2").boundingBox())!.x;
  for (const heading of await page
    .locator(".artist-shelf .home-shelf-heading h2")
    .all())
    expect(Math.abs((await heading.boundingBox())!.x - aboutLeft)).toBeLessThan(
      1,
    );
  // Popular rows carry badges, not artist names, like Spotify.
  await expect(
    page.locator(".artist-top .track-row .track-artists .artist-link"),
  ).toHaveCount(0);
  const card = page.locator(".artist-about-card");
  await expect(card.locator(".artist-rank")).toHaveText("#21in the world");
  await expect(card).toContainText("72,847,177 monthly listeners");
  await card.click();
  const dialog = page.getByRole("dialog", { name: "About Test artist" });
  // Spotify's About window is a compact sheet, not most of the screen.
  const sheet = (await dialog.boundingBox())!;
  expect(sheet.width).toBeLessThanOrEqual(900);
  // Its scroller uses the same slim overlay thumb as the panels.
  await expect(dialog).toHaveClass(/overlay-scrolled/);
  await page.screenshot({ path: "target/diagnostics/artist-about.png" });
  await expect(dialog.locator(".artist-fact")).toHaveText([
    "112,804,519Followers",
    "72,847,177Monthly Listeners",
  ]);
  await expect(dialog.locator(".artist-rank")).toHaveText("#21in the world");
  await expect(dialog.locator(".artist-city")).toHaveText(
    "London, GB1,076,135 listeners",
  );
  const photo = dialog.locator(".artist-about-gallery img");
  await expect(photo).toHaveAttribute("src", /one\.svg/);
  await expect(
    dialog.getByRole("button", { name: "Previous photo" }),
  ).toHaveCount(0);
  // Stats and biography sit side by side under the photo, like Spotify.
  const facts = (await dialog.locator(".artist-about-facts").boundingBox())!;
  const text = (await dialog.locator(".artist-about-text").boundingBox())!;
  expect(text.x).toBeGreaterThan(facts.x + facts.width);
  await page.screenshot({ path: "target/diagnostics/artist-about.png" });
  await dialog.getByRole("button", { name: "Next photo" }).click();
  await expect(photo).toHaveAttribute("src", /two\.svg/);
  await expect(
    dialog.getByRole("button", { name: "Previous photo" }),
  ).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  // A shelf's Show all opens it as a page of its own, like Spotify.
  await page
    .getByRole("region", { name: "Fans also like" })
    .getByRole("button", { name: "Show all" })
    .click();
  await expect(page.locator(".artist-shelf-page h1")).toHaveText(
    "Fans also like",
  );
  await expect(page.locator(".artist-shelf-page .home-card")).not.toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "Go back", exact: true }).click();
  await expect(page.locator(".artist-about > h2")).toBeVisible();

  // The sticky bar appears once the header scrolls away, in both themes.
  await page
    .locator(".browse-scroll")
    .evaluate((node) => node.scrollTo(0, 900));
  await expect(sticky).toHaveClass(/shown/);
  await expect(sticky).toContainText("Test artist");
  await page.evaluate(() =>
    document.querySelector(".application")!.classList.add("liquid-glass"),
  );
  await expect(sticky).toHaveCSS("opacity", "1");
  await page.screenshot({ path: "target/diagnostics/artist-sticky-glass.png" });
  await page.locator(".browse-scroll").evaluate((node) => node.scrollTo(0, 0));
  await expect(sticky).not.toHaveClass(/shown/);

  // Linked artists in the biography open their page.
  await page.locator(".artist-about-card").click();
  await page.getByRole("link", { name: "Mentor" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "browse")
          .at(-1)?.args.key,
    ),
  ).toBe("artist:0123456789abcdefghij07");
});

test("songs Spotify cannot play are dimmed and never start", async ({
  page,
}) => {
  await startClient(page);
  const browse = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1),
  );
  await page.evaluate(
    ({ generation, key, track }) =>
      (window as any).__clientTest.emit({
        type: "tracks",
        key,
        generation,
        offset: 0,
        more: false,
        tracks: [
          {
            ...track,
            uri: "spotify:track:gone",
            name: "Prompting",
            unavailable: true,
          },
          { ...track, uri: "spotify:track:here", name: "Still here" },
        ],
      }),
    { generation: browse.args.generation, key: browse.args.key, track },
  );
  const gone = page.locator(".track-row", { hasText: "Prompting" });
  await expect(gone).toHaveClass(/unavailable/);
  await expect(gone.locator(".track-cell")).toHaveCSS("opacity", "0.45");
  const players = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "player",
        ).length,
    );
  const before = await players();
  await gone.locator(".track-name").dblclick();
  await gone.hover();
  await expect(gone.locator(".row-play")).toBeHidden();
  expect(await players()).toBe(before);
  // Playing a neighbour leaves the unavailable song out of the list.
  await page
    .locator(".track-row", { hasText: "Still here" })
    .locator(".track-name")
    .dblclick();
  await expect.poll(players).toBeGreaterThan(before);
  const request = await page.evaluate(
    () =>
      (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "player")
        .at(-1).args.request,
  );
  expect(JSON.stringify(request)).not.toContain("spotify:track:gone");
});

test("a click highlights a song without a selection bar; a double click plays it", async ({
  page,
}) => {
  await startClient(page);
  const row = page.locator(".track-row").nth(1);
  const top = (await row.boundingBox())!.y;
  await row.locator(".track-name").click();
  await expect(page.locator(".track-row.selected")).toHaveCount(1);
  await expect(row).toHaveClass(/selected/);
  // Nothing appears above the list to push it down.
  await expect(page.locator(".selection-bar")).toHaveCount(0);
  expect((await row.boundingBox())!.y).toBe(top);
  const plays = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "player",
        ).length,
    );
  const before = await plays();
  await row.locator(".track-name").dblclick();
  await page.waitForTimeout(300);
  // A double-click loads the song once, not once per click.
  expect(await plays()).toBe(before + 1);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request.command,
      ),
    )
    .toBeTruthy();
});

test("Liked Songs names its context and its row turns green when playing", async ({
  page,
}) => {
  await startClient(page);
  const row = page.locator(".track-row").nth(1);
  const uri = await row.getAttribute("data-uri");
  await row.locator(".track-name").dblclick();
  const request = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "player")
          .at(-1)?.args.request,
    );
  // Named as Liked Songs, so the backend can play Spotify's collection.
  await expect.poll(request).toMatchObject({
    command: "play_uris",
    context: "liked",
  });
  const sent = await request();
  if (uri) expect(sent.uris[sent.offset]).toBe(uri);
  const context = "spotify:user:test-user:collection";
  await page.evaluate(
    ({ base, track, context }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...base, item: track, is_playing: true, context },
        observed_at: Date.now(),
        changed: true,
        loading: true,
      }),
    { base: emptyState.playback, track, context },
  );
  const liked = page.locator(".library-liked");
  await expect(liked).toHaveClass(/playing/);
  await expect(liked.locator(".library-item strong")).toHaveCSS(
    "color",
    "rgb(30, 215, 96)",
  );
  await expect(liked.getByLabel("Playing")).toBeVisible();
});

test("clicking an artist in a song row opens it without selecting the song", async ({
  page,
}) => {
  await startClient(page);
  await page.locator(".track-row .track-artists .artist-link").first().click();
  await expect(page.locator(".selection-bar")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "browse")
          .at(-1)?.args.key,
    ),
  ).toMatch(/^artist:/);
});

test("songs select like Spotify: Ctrl, Shift, Ctrl+A, right-click and Escape", async ({
  page,
}) => {
  await startClient(page);
  const rows = page.locator(".track-row");
  const chosen = page.locator(".track-row.selected");
  // A click on the row's blank space selects it alone.
  await rows.nth(0).click({ position: { x: 400, y: 4 } });
  await expect(chosen).toHaveCount(1);
  await rows.nth(2).click({ modifiers: ["Shift"], position: { x: 400, y: 4 } });
  await expect(chosen).toHaveCount(3);
  await rows
    .nth(1)
    .click({ modifiers: ["Control"], position: { x: 400, y: 4 } });
  await expect(chosen).toHaveCount(2);
  await expect(rows.nth(1)).not.toHaveClass(/selected/);
  // Ctrl+A takes every listed song once one is selected.
  await page.keyboard.press("Control+a");
  await expect(chosen).toHaveCount(await rows.count());
  const background = await rows
    .nth(0)
    .evaluate((node) => getComputedStyle(node).backgroundColor);
  expect(background).toBe("rgba(255, 255, 255, 0.3)");
  // Right-clicking a selected song acts on the whole selection.
  await rows.nth(1).click({ button: "right", position: { x: 400, y: 4 } });
  const menu = page.getByRole("menu", { name: "Song actions" });
  await expect(
    menu.getByRole("menuitem", { name: "Copy song IDs" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(chosen).toHaveCount(0);
  // Right-clicking an unselected song highlights it alone.
  await rows.nth(3).click({ button: "right", position: { x: 400, y: 4 } });
  await expect(chosen).toHaveCount(1);
  await expect(rows.nth(3)).toHaveClass(/selected/);
  await expect(
    menu.getByRole("menuitem", { name: "Copy song ID" }),
  ).toBeVisible();
});

test("song rows link only the album name and underline artist and album, not the title", async ({
  page,
}) => {
  await startClient(page);
  const row = page.locator(".track-row").nth(1);
  const album = row.locator(".album-link");
  const width = await album.evaluate((node) => {
    const range = document.createRange();
    range.selectNodeContents(node);
    return {
      link: node.getBoundingClientRect().width,
      text: range.getBoundingClientRect().width,
    };
  });
  expect(width.link).toBeLessThanOrEqual(width.text + 1);
  const underline = async (locator: Locator) => {
    await locator.hover();
    return locator.evaluate(
      (node) => getComputedStyle(node).textDecorationLine,
    );
  };
  expect(await underline(row.locator(".track-name"))).toBe("none");
  expect(await underline(row.locator(".artist-link").first())).toBe(
    "underline",
  );
  expect(await underline(album)).toBe("underline");
});

test("Load TTML offers Spicy Lyrics' load modes, reset and saved lyrics", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.getByTitle("Load TTML", { exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Load TTML" });
  await expect(dialog).toContainText("For: Timing test");
  // The title sits level with the close button.
  const heading = (await dialog.locator(".load-ttml-head h2").boundingBox())!;
  const close = (await dialog
    .getByRole("button", { name: "Close" })
    .boundingBox())!;
  expect(
    Math.abs(heading.y + heading.height / 2 - (close.y + close.height / 2)),
  ).toBeLessThan(2);
  await expect(dialog.locator(".load-ttml-mode strong")).toHaveText([
    "Persistent Load",
    "Temporary Load",
    "Session Load",
  ]);
  const last = (command: string) =>
    page.evaluate(
      (command) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === command)
          .at(-1)?.args,
      command,
    );
  const chooser = page.waitForEvent("filechooser");
  await dialog.getByRole("button", { name: /Session Load/ }).click();
  await (
    await chooser
  ).setFiles({
    name: "song.ttml",
    mimeType: "application/ttml+xml",
    buffer: Buffer.from(
      '<tt><body><div><p begin="1s" end="2s">Hi</p></div></body></tt>',
    ),
  });
  await expect
    .poll(() => last("import_lyrics"))
    .toMatchObject({
      filename: "song.ttml",
      mode: "session",
    });
  await expect(dialog).toHaveCount(0);

  await page.getByTitle("Load TTML", { exact: true }).click();
  await dialog.getByRole("button", { name: "TTML Database" }).click();
  const database = page.getByRole("dialog", { name: "TTML Database" });
  await expect(database).toContainText("No saved lyrics yet");
  // The title lines up with the close button, and Back, search and Reset
  // share one row, as in Spicy Lyrics.
  const middle = async (locator: any) => {
    const box = (await locator.boundingBox())!;
    return box.y + box.height / 2;
  };
  const title = await middle(database.locator(".load-ttml-head h2"));
  expect(
    Math.abs(
      title - (await middle(database.getByRole("button", { name: "Close" }))),
    ),
  ).toBeLessThan(2);
  const back = await middle(
    database.getByRole("button", { name: "Back to Load TTML" }),
  );
  for (const control of [
    database.getByLabel("Search saved lyrics"),
    database.getByRole("button", { name: "Reset TTML" }),
  ])
    expect(Math.abs((await middle(control)) - back)).toBeLessThan(2);
  await database.getByRole("button", { name: "Back to Load TTML" }).click();
  await dialog.getByRole("button", { name: "Reset TTML" }).click();
  await expect.poll(() => last("reset_lyrics")).not.toBeUndefined();
  await expect(dialog).toHaveCount(0);
  await page.evaluate(() =>
    document.querySelector(".application")!.classList.add("liquid-glass"),
  );
  await page.getByTitle("Load TTML", { exact: true }).click();
  // Glass: a translucent sheet over a backdrop that dims without blurring,
  // so the ambience shows through.
  await expect(page.locator(".load-ttml-backdrop")).toHaveCSS(
    "backdrop-filter",
    "none",
  );
  const fill = await dialog.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  );
  expect(Number(fill.match(/[\d.]+(?=\)$)/)?.[0] ?? 1)).toBeLessThan(0.6);
  await page.screenshot({ path: "target/diagnostics/load-ttml-glass.png" });
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("playlists show Date added and sort by oldest added", async ({ page }) => {
  await startClient(page);
  await expect(page.locator(".dated-table .track-table-head")).toContainText(
    "Date added",
  );
  await page.evaluate(
    ({ track, generation }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "tracks",
        key: "liked",
        generation,
        offset: 0,
        more: false,
        tracks: [
          {
            ...track,
            uri: "spotify:track:new",
            name: "Newer",
            added_at: new Date(Date.now() - 3 * 86_400_000).toISOString(),
          },
          {
            ...track,
            uri: "spotify:track:old",
            name: "Older",
            added_at: "2024-03-13T00:54:46Z",
          },
        ],
      });
    },
    {
      track,
      generation: await page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1).args.generation,
      ),
    },
  );
  const rows = page.locator(".track-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0).locator(".track-added")).toHaveText("3 days ago");
  await expect(rows.nth(1).locator(".track-added")).toContainText("2024");
  await page.getByTitle("Sort and filter").click();
  await page.getByRole("button", { name: "Oldest added" }).click();
  await expect(rows.nth(0)).toContainText("Older");
});

test("song row covers load a small Spotify size without sizes=auto", async ({
  page,
}) => {
  // Cover files are not fetched in this test; only the chosen URL matters.
  await page.route("https://i.scdn.co/**", (route) => route.abort());
  await startClient(page);
  const id = "b11c3ab5cb818192b7a61fdd";
  await page.evaluate(
    ({ track, id, generation }) => {
      (window as any).__clientTest.emit({
        type: "tracks",
        key: "liked",
        generation,
        offset: 0,
        more: false,
        tracks: [
          {
            ...track,
            album: {
              ...track.album,
              images: [
                { url: `https://i.scdn.co/image/ab67616d0000b273${id}` },
              ],
            },
          },
        ],
      });
    },
    {
      track,
      id,
      generation: await page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1).args.generation,
      ),
    },
  );
  const cover = page.locator(".track-row .cover img").first();
  // A 40-pixel row cover takes Spotify's 64-pixel file, not the 640 one.
  await expect(cover).toHaveAttribute(
    "src",
    `https://i.scdn.co/image/ab67616d00004851${id}`,
  );
  // sizes="auto" keeps removed images alive in Chromium; never use it.
  await expect(page.locator("img[sizes]")).toHaveCount(0);
});

for (const route of ["liked", "playlist:0123456789abcdefghijkl"]) {
  test(`long collection ${route.split(":")[0]} stays visible after leaving lyrics`, async ({
    page,
  }) => {
    await startClient(page, route);
    await expect(page.locator(".track-row")).toHaveCount(150);
    await page.evaluate(
      ({ track }) => {
        const host = (window as any).__clientTest;
        const browse = host.commands
          .filter((entry: any) => entry.command === "browse")
          .at(-1).args;
        host.emit({
          type: "tracks",
          key: browse.key,
          generation: browse.generation,
          offset: 0,
          more: false,
          tracks: Array.from({ length: 804 }, (_, index) => ({
            ...track,
            uri: `spotify:track:long${index}`,
            name: `Long song ${index + 1}`,
          })),
        });
      },
      { track },
    );
    const scroller = page.locator(".browse-scroll");
    await scroller.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect(
      page.getByText("Long song 804", { exact: true }),
    ).toBeVisible();
    await page.getByTitle("Lyrics", { exact: true }).click();
    await expect(page.locator(".lyrics-page")).toBeVisible();
    await page.getByTitle("Lyrics", { exact: true }).click();
    await expect(page.getByText("Long song 1", { exact: true })).toBeVisible();
    expect(await page.locator(".track-row").count()).toBeLessThan(120);
    await page.locator(".browse-scroll").evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect(
      page.getByText("Long song 804", { exact: true }),
    ).toBeVisible();
  });
}

test("public playlist outside the library shows its creator name and avatar", async ({
  page,
}) => {
  await startClient(page, "playlist:0123456789abcdefghijkl");
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const browse = host.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1).args;
    host.emit({
      type: "browse_info",
      generation: browse.generation,
      info: {
        key: browse.key,
        title: "Public playlist",
        subtitle: "A public collection",
        images: [],
        albums: [],
        playlist: {
          owner: "public-owner-id",
          total_tracks: 150,
          format: "",
          owner_profile: {
            id: "public-owner-id",
            display_name: "Maya Chen",
            images: [
              { url: "https://artwork.example.test/public-owner-avatar" },
            ],
          },
        },
      },
    });
  });
  await expect(page.locator(".hero-meta")).toContainText("Maya Chen");
  await expect(page.locator(".hero-meta")).not.toContainText("public-owner-id");
  await expect(page.locator(".hero-owner")).toBeVisible();
  await expect
    .poll(() =>
      page
        .locator(".hero-owner")
        .evaluate((image: HTMLImageElement) => image.naturalWidth),
    )
    .toBeGreaterThan(0);
});

test("playlist creator profile cannot replace a different owner", async ({
  page,
}) => {
  await startClient(page, "playlist:0123456789abcdefghijkl");
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const browse = host.commands
      .filter((entry: any) => entry.command === "browse")
      .at(-1).args;
    host.emit({
      type: "browse_info",
      generation: browse.generation,
      info: {
        key: browse.key,
        title: "Public playlist",
        subtitle: "",
        images: [],
        albums: [],
        playlist: {
          owner: "public-owner-id",
          total_tracks: 150,
          format: "",
          owner_profile: {
            id: "different-owner",
            display_name: "Unrelated person",
            images: [{ url: "https://artwork.example.test/unrelated-avatar" }],
          },
        },
      },
    });
  });
  await expect(page.locator(".hero-meta")).toContainText("public-owner-id");
  await expect(page.locator(".hero-meta")).not.toContainText(
    "Unrelated person",
  );
  await expect(page.locator(".hero-owner")).toHaveCount(0);
});

test("long playlists render only the rows near the viewport", async ({
  page,
}) => {
  await startClient(page);
  const count = 800;
  await page.evaluate(
    ({ track, count }) => {
      const host = (window as any).__clientTest;
      const browse = host.commands
        .filter((entry: any) => entry.command === "browse")
        .at(-1).args;
      host.emit({
        type: "tracks",
        key: browse.key,
        generation: browse.generation,
        offset: 0,
        more: false,
        tracks: Array.from({ length: count }, (_, index) => ({
          ...track,
          id: `long${index}`,
          uri: `spotify:track:long${index}`,
          name: `Long song ${index + 1}`,
        })),
      });
    },
    { track, count },
  );
  const rows = page.locator(".track-row");
  await expect(rows.first()).toContainText("Long song 1");
  // Every song is loaded, but only a window of rows is in the page.
  expect(await rows.count()).toBeLessThan(120);
  const scroller = page.locator(".browse-scroll");
  // The list keeps its full height so the scrollbar matches 800 songs.
  expect(
    await scroller.evaluate((element) => element.scrollHeight),
  ).toBeGreaterThan(count * 50);
  await scroller.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await expect(page.getByText("Long song 800", { exact: true })).toBeVisible();
  expect(await rows.count()).toBeLessThan(120);
  // Rows keep their place in the list for playback and selection.
  await expect(
    page
      .locator(".track-row", { hasText: "Long song 800" })
      .locator(".track-number"),
  ).toContainText("800");
});

test("Settings shows the song cache size and clears it", async ({ page }) => {
  await startClient(page);
  await page.evaluate(
    () => ((window as any).__clientTest.cacheBytes = 734_003_200),
  );
  await openSettings(page);
  await expect(page.getByText("Cache: 700 MB")).toBeVisible();
  await page.getByRole("button", { name: "Clear cache" }).click();
  await expect(page.getByText("Cache: 0 MB")).toBeVisible();
  await expect(page.getByText("Cleared 700 MB of cached songs.")).toBeVisible();
});

test("updates can be downloaded manually before installing and restarting", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page);
  await page
    .getByRole("switch", {
      name: "Download updates automatically",
      exact: true,
    })
    .uncheck();
  const action = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "app_update")
          .at(-1)?.args.action,
    );
  await page
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await expect.poll(action).toBe("check");
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "update",
      status: "Update available",
      version: "0.2.0",
    }),
  );
  await page
    .getByRole("dialog", { name: "Lyssna 0.2.0 is available" })
    .getByRole("button", { name: "Download update", exact: true })
    .click();
  await expect.poll(action).toBe("download");
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "update",
      status: "Ready to install",
      version: "0.2.0",
    }),
  );
  await page
    .getByRole("dialog", { name: "Lyssna 0.2.0 is available" })
    .getByRole("button", { name: "Install and restart", exact: true })
    .click();
  await expect.poll(action).toBe("install");
});

test("startup updates prompt automatically, keep notes inert and respect Not now", async ({
  page,
}) => {
  await startClient(page);
  const notes =
    'New features. <img src="https://example.test/tracker" onerror="alert(1)">';
  const offer = (version: string) =>
    page.evaluate(
      ({ version, notes }) =>
        (window as any).__clientTest.emit({
          type: "update",
          status: "Ready to install",
          version,
          notes,
        }),
      { version, notes },
    );
  await offer("0.2.0");
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText(`You're using version ${appVersion}`);
  await expect(
    dialog.getByRole("region", { name: "Release notes" }),
  ).toContainText(notes);
  await expect(dialog.locator("img")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "app_update",
        ).length,
    ),
  ).toBe(0);
  for (let count = 0; count < 4; count++) {
    await page.keyboard.press("Tab");
    expect(
      await dialog.evaluate((element) =>
        element.contains(document.activeElement),
      ),
    ).toBe(true);
  }
  // Space activates modal controls rather than the global playback shortcut.
  await dialog.getByRole("button", { name: "Not now" }).focus();
  await page.keyboard.press("Space");
  await expect(dialog).toBeHidden();
  await offer("0.2.0");
  await expect(dialog).toBeHidden();
  await offer("0.3.0");
  await expect(dialog).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
});

test("Your Library covers show a play button for playlists and Liked Songs", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "library",
      playlists: [
        {
          id: "4123456789abcdefghijkl",
          name: "Road trip",
          images: [],
          owner: { id: "test-user" },
          collaborative: false,
        },
      ],
    }),
  );
  const lastPlayer = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "player")
          .at(-1)?.args.request,
    );
  const route = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "browse")
          .at(-1)?.args.key,
    );
  const before = await route();
  const row = page.locator(
    '.library-row[data-playlist-id="4123456789abcdefghijkl"]',
  );
  const playRoadTrip = row.getByRole("button", { name: "Play Road trip" });
  await expect(playRoadTrip).toHaveCSS("opacity", "0");
  await row.hover();
  await expect(playRoadTrip).toHaveCSS("opacity", "1");
  await playRoadTrip.click();
  // Starts the playlist without leaving the current page.
  await expect.poll(lastPlayer).toMatchObject({
    command: "play_home",
    context: "spotify:playlist:4123456789abcdefghijkl",
  });
  expect(await route()).toBe(before);
  // DJ starts from its own row; it has no cover play button.
  await expect(page.getByRole("button", { name: "Play DJ" })).toHaveCount(0);
  // From another page, Liked Songs opens and starts its first song.
  await page.locator(".home-button").click();
  await expect.poll(route).toBe("home");
  await page.locator(".library-liked").hover();
  await page
    .locator(".library-cover-play[aria-label='Play Liked Songs']")
    .click();
  await expect.poll(route).toBe("liked");
  await expect.poll(lastPlayer).toMatchObject({
    command: "play_uris",
    context: "liked",
  });
});

test("lyrics toolbar sits over the cover art and follows it when flipped", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-album")).toBeVisible();
  const layout = () =>
    page.evaluate(() => {
      const box = (selector: string) =>
        document.querySelector(selector)!.getBoundingClientRect();
      const page = box(".lyrics-page");
      const album = box(".lyrics-album");
      const toolbar = box(".lyrics-toolbar");
      return {
        albumRight: album.left > page.left + page.width / 2,
        // One row of buttons, never wrapped.
        oneRow: toolbar.height < 60,
        toolbarRight: toolbar.left > page.left + page.width / 2,
        // Stays over the artwork column, not the lyrics.
        overArtwork:
          toolbar.left >=
            page.left +
              (toolbar.left > page.left + page.width / 2
                ? page.width / 2
                : 0) &&
          (toolbar.left > page.left + page.width / 2
            ? toolbar.left >= album.left - 40
            : toolbar.right <= album.right + 40),
      };
    });
  await expect.poll(layout).toEqual({
    albumRight: false,
    oneRow: true,
    toolbarRight: false,
    overArtwork: true,
  });
  // The flip animates: the browser's transition starts and is not skipped.
  await page.evaluate(() => {
    const original = (document as any).startViewTransition.bind(document);
    (window as any).__transitions = [];
    (document as any).startViewTransition = (callback: () => void) => {
      const transition = original(callback);
      (window as any).__transitions.push(
        transition.ready.then(
          () => "ran",
          (error: Error) => error.name,
        ),
      );
      return transition;
    };
  });
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Artwork on the right", { exact: true })
    .click();
  expect(
    await page.evaluate(() => Promise.all((window as any).__transitions)),
  ).toEqual(["ran"]);
  await expect.poll(layout).toEqual({
    albumRight: true,
    oneRow: true,
    toolbarRight: true,
    overArtwork: true,
  });
});

test("Home artist cards open with their name and photo at once and prefetch on hover", async ({
  page,
}) => {
  await startClient(page, "home");
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "home",
      page: {
        account: "test-user",
        greeting: "Hello",
        cached: false,
        sections: [
          {
            uri: "artists",
            title: "Your favourite artists",
            subtitle: "",
            items: [
              {
                uri: "spotify:artist:3123456789abcdefghijkl",
                route: "artist:3123456789abcdefghijkl",
                name: "Card artist",
                subtitle: "Artist",
                images: [
                  { url: "https://artwork.example.test/card-artist.svg" },
                ],
                kind: "artist",
                playable: true,
                format: "",
              },
            ],
          },
        ],
      },
    }),
  );
  // A one-card section shows large, as Spotify's featured shelf.
  const card = page.getByRole("region", { name: "Your favourite artists" });
  await card.hover();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (entry: any) =>
            entry.command === "prefetch_page" &&
            entry.args.key === "artist:3123456789abcdefghijkl",
        ),
      ),
    )
    .toBe(true);
  await card.getByRole("button", { name: "Card artist", exact: true }).click();
  // Spotify has not answered yet: the card's own name and photo show.
  await expect(page.locator(".hero-copy h1")).toHaveText("Card artist");
  await expect(page.locator(".hero-cover img")).toHaveAttribute(
    "src",
    /card-artist\.svg/,
  );
});

test("TTML Database lists covers and plays, downloads and filters saved lyrics", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    (window as any).__clientTest.savedLyrics = [
      {
        key: "a".repeat(64),
        uri: "spotify:track:0123456789abcdefghijk1",
        name: "Saved song",
        artists: "Artist",
        kind: "ttml",
        saved_at: 2,
        image: "https://artwork.example.test/saved.svg",
      },
      {
        key: "b".repeat(64),
        uri: "spotify:track:0123456789abcdefghijk2",
        name: "Other tune",
        artists: "Someone",
        kind: "lrc",
        saved_at: 1,
      },
    ];
  });
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.getByTitle("Load TTML", { exact: true }).click();
  await page
    .getByRole("dialog", { name: "Load TTML" })
    .getByRole("button", { name: "TTML Database" })
    .click();
  const database = page.getByRole("dialog", { name: "TTML Database" });
  const rows = database.locator(".load-ttml-saved li");
  await expect(rows).toHaveCount(2);
  // The list fades out at the bottom instead of cutting rows off.
  expect(
    await database
      .locator(".load-ttml-saved")
      .evaluate((list) => getComputedStyle(list).maskImage),
  ).toContain("linear-gradient");
  await expect(rows.first().locator("img")).toHaveAttribute(
    "src",
    /saved\.svg/,
  );
  // A song without known art keeps a placeholder of the same size.
  await expect(rows.nth(1).locator(".load-ttml-art")).toBeVisible();
  await database.getByRole("button", { name: "Play Saved song" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request,
      ),
    )
    .toMatchObject({
      command: "play",
      uri: "spotify:track:0123456789abcdefghijk1",
    });
  const download = page.waitForEvent("download");
  await database
    .getByRole("button", { name: "Download lyrics for Saved song" })
    .click();
  expect((await download).suggestedFilename()).toBe("Artist - Saved song.ttml");
  await page.evaluate(() =>
    document.querySelector(".application")!.classList.add("liquid-glass"),
  );
  await page.screenshot({ path: "target/diagnostics/ttml-database.png" });
  await database.getByLabel("Search saved lyrics").fill("other");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("Other tune");
});

test("compact lyrics run the toolbar down the right edge", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Compact lyrics", { exact: true })
    .click();
  const layout = () =>
    page.evaluate(() => {
      const box = (selector: string) =>
        document.querySelector(selector)!.getBoundingClientRect();
      const toolbar = box(".lyrics-toolbar");
      const album = box(".lyrics-album");
      const page = box(".lyrics-page");
      const lines = document
        .querySelector(".lyrics-body")!
        .getBoundingClientRect();
      return {
        // A column at the right edge, top to bottom.
        vertical: toolbar.height > toolbar.width * 3,
        atRight: page.right - toolbar.right < 20,
        // Clear of the song header and the lyrics.
        clearOfHeader: toolbar.left >= album.right - 1,
        clearOfLyrics: toolbar.left >= lines.right - 1,
        // The header still gets most of the width.
        wideHeader: album.width > page.width * 0.7,
      };
    });
  await expect.poll(layout).toEqual({
    vertical: true,
    atRight: true,
    clearOfHeader: true,
    clearOfLyrics: true,
    wideHeader: true,
  });
  await page.waitForTimeout(500);
  await page.mouse.move(800, 500);
  await page.mouse.move(820, 520);
  await page.screenshot({ path: "target/diagnostics/compact-lyrics.png" });
});

test("lyrics toolbar fades in on pointer movement and out a second later", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const toolbar = page.getByRole("toolbar", { name: "Lyrics" });
  const lyrics = page.locator(".lyrics-page");
  const opacity = () =>
    toolbar.evaluate((element) => Number(getComputedStyle(element).opacity));
  const box = (await lyrics.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width / 2 + 20, box.y + box.height / 2);
  await expect.poll(opacity).toBe(1);
  // Still pointer: it fades after a second.
  await expect.poll(opacity, { timeout: 4000 }).toBe(0);
  // Resting on the toolbar keeps it, darkened but still see-through.
  await toolbar.hover();
  await expect.poll(opacity).toBe(1);
  await page.waitForTimeout(1300);
  expect(await opacity()).toBe(1);
  const background = await toolbar.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  );
  const alpha = Number(background.match(/[\d.]+(?=\)$)/)?.[0] ?? 1);
  expect(alpha).toBeGreaterThan(0.2);
  expect(alpha).toBeLessThan(0.6);
  // Leaving the lyrics hides it at once.
  await page.mouse.move(5, 5);
  await expect.poll(opacity).toBe(0);
});

test("closing and opening Now playing animates the panel", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.evaluate(() => {
    const original = (document as any).startViewTransition.bind(document);
    (window as any).__transitions = [];
    (document as any).startViewTransition = (callback: () => void) => {
      const transition = original(callback);
      (window as any).__transitions.push(
        transition.ready.then(
          () => "ran",
          (error: Error) => error.name,
        ),
      );
      return transition;
    };
  });
  const toggle = page
    .locator(".player-bar")
    .getByTitle("Now playing", { exact: true });
  await expect(page.locator("aside.now-playing")).toHaveCount(1);
  await toggle.click();
  await expect(page.locator("aside.now-playing")).toHaveCount(0);
  await toggle.click();
  await expect(page.locator("aside.now-playing")).toHaveCount(1);
  expect(
    await page.evaluate(() => Promise.all((window as any).__transitions)),
  ).toEqual(["ran", "ran"]);
});

test("lyrics toolbar icons animate their parts on hover", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const toolbar = page.getByRole("toolbar", { name: "Lyrics" });
  const scaleOf = (selector: string) =>
    page.evaluate((selector) => {
      const value = getComputedStyle(document.querySelector(selector)!).scale;
      return value === "none" ? 1 : Number(value.split(" ")[0]);
    }, selector);
  const front = async () =>
    (await scaleOf("[data-icon='romanized'] .part-symbol")) >
    (await scaleOf("[data-icon='romanized'] .part-latin"))
      ? "symbol"
      : "latin";
  // The A is in front until hovered; romanizing keeps the symbol in front.
  await page.mouse.move(5, 5);
  await expect.poll(front).toBe("latin");
  const translate = toolbar.getByTitle("Romanization", { exact: true });
  await translate.hover();
  await expect.poll(front).toBe("symbol");
  await translate.click();
  await page.mouse.move(5, 5);
  await expect.poll(front).toBe("symbol");
  await translate.hover();
  await translate.click();
  await page.mouse.move(5, 5);
  await expect.poll(front).toBe("latin");
  // The other icons run their own animation while hovered.
  for (const [icon, part] of [
    ["upload", ".part-arrow"],
    ["retry", ".part-spinner"],
    ["fullscreen", ".part-corner-out"],
    ["lyrics_artwork_right", ".part-left"],
    ["experimental_word_sync", ".part-bar"],
  ]) {
    await toolbar.locator(`[data-icon="${icon}"]`).hover();
    await expect
      .poll(() =>
        page.evaluate(
          ({ icon, part }) =>
            document
              .querySelector(`[data-icon="${icon}"] ${part}`)!
              .getAnimations().length,
          { icon, part },
        ),
      )
      .toBeGreaterThan(0);
  }
});

test("the top-left menu opens File, Edit, View, Playback and Help", async ({
  page,
}) => {
  await startClient(page);
  await expect(page.locator(".topbar .logo")).toHaveCount(0);
  const menu = page.getByRole("button", { name: "Menu", exact: true });
  await menu.click();
  const groups = page.getByRole("menu", { name: "Menu", exact: true });
  await expect(groups.getByRole("menuitem")).toHaveText([
    "File",
    "Edit",
    "View",
    "Playback",
    "Help",
  ]);
  const commands = (name: string) =>
    page.evaluate(
      (name) =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === name,
        ),
      name,
    );
  await groups.getByRole("menuitem", { name: "View" }).hover();
  await page
    .getByRole("menu", { name: "View" })
    .getByRole("menuitem", { name: "Zoom in" })
    .click();
  await expect(groups).toHaveCount(0);
  await expect
    .poll(
      async () =>
        (await commands("save_settings")).at(-1)?.args.settings.ui_zoom_percent,
    )
    .toBe(110);
  await menu.click();
  await page.getByRole("menuitem", { name: "Playback" }).click();
  await page
    .getByRole("menu", { name: "Playback" })
    .getByRole("menuitem", { name: "Next" })
    .click();
  await expect
    .poll(async () => (await commands("player")).at(-1)?.args.request.command)
    .toBe("next");
  await menu.click();
  await page.getByRole("menuitem", { name: "Edit" }).click();
  await page
    .getByRole("menu", { name: "Edit" })
    .getByRole("menuitem", { name: "Settings" })
    .click();
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  // Clicking elsewhere closes it.
  await menu.click();
  await page.locator(".center-panel").click({ position: { x: 300, y: 300 } });
  await expect(page.getByRole("menu", { name: "Menu" })).toHaveCount(0);
});

test("Help > About Lyssna opens the About dialog", async ({ page }) => {
  await startClient(page);
  await page.getByRole("button", { name: "Menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "Help" }).click();
  await page
    .getByRole("menu", { name: "Help" })
    .getByRole("menuitem", { name: "About Lyssna" })
    .click();
  const about = page.getByRole("dialog", { name: "About Lyssna" });
  await expect(about).toBeVisible();
  await expect(about).toContainText("Lyssna for Windows (64 bit)");
  await expect(about.locator(".about-legal")).toContainText("Copyright");
  await about.getByRole("button", { name: "Close", exact: true }).click();
  await expect(about).toHaveCount(0);
  await page.getByRole("button", { name: "Menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "Help" }).click();
  await page.getByRole("menuitem", { name: "About Lyssna" }).click();
  await expect(about).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(about).toHaveCount(0);
});

test("Lyssna settings combines app and lyrics options with ordered sources", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page);
  const dialog = page.getByRole("dialog", { name: "Lyssna settings" });
  await expect(dialog).toBeVisible();
  await expect(page.locator(".settings-page")).toHaveCount(0);
  await expect(
    dialog.getByRole("combobox", { name: "Audio output" }),
  ).toBeVisible();
  await expect(page.locator(".track-row").first()).toBeVisible();
  await expect(page.getByLabel("Lyrics font", { exact: true })).toHaveCount(0);
  const saved = (key: string) =>
    page.evaluate(
      (key) =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings[key],
      key,
    );
  await openSettings(page, "Sources");
  const sources = page.locator(".lyric-sources li");
  await expect(sources).toHaveText([
    /Spicy Lyrics/,
    /Apple Music/,
    /Musixmatch/,
    /Spotify/,
    /LRCLIB/,
    /Netease/,
  ]);
  await expect(
    page.getByRole("switch", { name: "Musixmatch", exact: true }),
  ).toBeChecked();
  await page.getByRole("switch", { name: "LRCLIB", exact: true }).check();
  await expect
    .poll(() => saved("lyrics_sources_disabled"))
    .toEqual(["netease"]);
  await page.getByTitle("Move Spotify up", { exact: true }).click();
  await expect
    .poll(() => saved("lyrics_source_order"))
    .toEqual(["spicy", "apple", "spotify", "musixmatch", "lrclib", "netease"]);
  await page.getByRole("tab", { name: "Lyrics", exact: true }).click();
  await page.getByRole("switch", { name: "Right align lyrics" }).check();
  await page.getByRole("switch", { name: "Simple lyrics mode" }).check();
  await expect.poll(() => saved("lyrics_simple")).toBe(true);
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await expect(page.locator(".lyrics-display")).toHaveClass(/lyrics-right/);
  await openSettings(page, "Appearance");
  await dialog.getByRole("switch", { name: "Liquid Glass" }).check();
  await dialog.getByRole("tab", { name: "Lyrics", exact: true }).click();
  await dialog.screenshot({
    path: "target/diagnostics/lyssna-settings-glass.png",
    animations: "disabled",
  });
  // Every category fits the smallest supported window.
  await page.setViewportSize({ width: 900, height: 600 });
  for (const tab of [
    "General",
    "Appearance",
    "Library",
    "Lyrics",
    "Effects",
    "Layout",
    "Sources",
  ]) {
    await dialog.getByRole("tab", { name: tab, exact: true }).click();
    expect(
      await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth),
    ).toBe(true);
    expect(
      await dialog
        .locator(".lyrics-settings-scroll")
        .evaluate((node) => node.scrollWidth <= node.clientWidth),
    ).toBe(true);
  }
  await dialog.screenshot({
    path: "target/diagnostics/lyssna-settings-small.png",
    animations: "disabled",
  });
  await page.mouse.click(5, 300);
  await expect(dialog).toHaveCount(0);
  await expect(page.locator(".lyrics-page")).toBeVisible();
});

for (const route of ["liked", "playlist:0123456789abcdefghijkl"]) {
  for (const [column, clicks, order] of [
    ["Album", 2, [1, 2, 0]],
    ["Date added", 1, [2, 1, 0]],
  ] as const) {
    test(`${route} queues visible ${column} order after sorting`, async ({
      page,
    }) => {
      await startClient(page, route);
      await expect(page.locator(".track-row")).toHaveCount(150);
      const tracks = ["Alpha", "Charlie", "Bravo"].map((album, index) => ({
        ...track,
        uri: `spotify:track:${String(index).padStart(22, "0")}`,
        name: index === 0 ? "Other song" : `Queue song ${index + 1}`,
        album: { ...track.album, name: album },
        added_at: `202${6 - index}-01-01T00:00:00Z`,
      }));
      await page.evaluate(
        ({ tracks, route }) => {
          const host = (window as any).__clientTest;
          const generation = host.commands
            .filter(
              (entry: any) =>
                entry.command === "browse" && entry.args.key === route,
            )
            .at(-1).args.generation;
          host.emit({
            type: "tracks",
            key: route,
            generation,
            offset: 0,
            more: false,
            tracks,
          });
        },
        { tracks, route },
      );
      await expect(page.locator(".track-row")).toHaveCount(3);
      const header = page.getByRole("button", {
        name: new RegExp(`Sort by ${column}`),
      });
      for (let click = 0; click < clicks; click++) await header.click();
      await expect(page.locator(".track-row .track-name")).toHaveText(
        order.map((index) => tracks[index].name),
      );
      await page.locator(".track-row .track-name").first().click();
      const request = () =>
        page.evaluate(
          () =>
            (window as any).__clientTest.commands
              .filter((entry: any) => entry.command === "player")
              .at(-1).args.request,
        );
      const expected = {
        command: "play_uris",
        context: route === "liked" ? "liked" : `spotify:${route}`,
        uris: order.map((index) => tracks[index].uri),
        offset: 0,
      };
      expect(await request()).toMatchObject(expected);
      // Starting from the middle retains the full list and its exact offset.
      await page.locator(".track-row .track-name").nth(1).click();
      expect(await request()).toMatchObject({ ...expected, offset: 1 });
      // The page's Play button starts the first displayed song too.
      await page.locator(".browse-actions .primary-play").click();
      expect(await request()).toMatchObject(expected);
      await page.getByTitle("Search in playlist", { exact: true }).click();
      await page.getByLabel("Search songs in this playlist").fill("Queue song");
      await expect(page.locator(".track-row")).toHaveCount(2);
      await page.locator(".browse-actions .primary-play").click();
      const filtered = {
        ...expected,
        uris: order
          .filter((index) => index !== 0)
          .map((index) => tracks[index].uri),
      };
      expect(await request()).toMatchObject(filtered);
      await page
        .locator(".track-row")
        .first()
        .getByTitle("Song options")
        .click();
      await page
        .getByRole("menu", { name: "Song actions" })
        .getByRole("menuitem", { name: "Play", exact: true })
        .click();
      expect(await request()).toMatchObject(filtered);
    });
  }
}

test("Title, Album and Date added headers sort like Spotify", async ({
  page,
}) => {
  await startClient(page);
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  await expect(page.locator(".track-row").first()).toBeVisible();
  const saved = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings.song_sorts?.liked ?? "order",
    );
  const first = () =>
    page.locator(".track-row .track-name").first().innerText();
  const head = page.locator(".track-table-head");
  const title = head.getByRole("button", { name: /Sort by Title/ });
  await title.click();
  await expect.poll(saved).toBe("title_asc");
  await expect(title).toHaveClass(/active/);
  const ascending = await first();
  await title.click();
  await expect.poll(saved).toBe("title_desc");
  await expect.poll(first).not.toBe(ascending);
  await title.click();
  await expect.poll(saved).toBe("order");
  await expect(title).not.toHaveClass(/active/);
  await head.getByRole("button", { name: /Sort by Album/ }).click();
  await expect.poll(saved).toBe("album_asc");
  const added = head.getByRole("button", { name: /Sort by Date added/ });
  await added.click();
  await expect.poll(saved).toBe("added_oldest");
  await added.click();
  await expect.poll(saved).toBe("recent");
});

test("custom header fonts retain italic overhang and artist descenders", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.evaluate(
    ({ base, track, settings }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "settings",
        settings: { ...settings, lyric_font: "hemi head" },
      });
      host.emit({
        type: "playback",
        playback: {
          ...base,
          item: {
            ...track,
            name: "ALL THE TIME",
            artists: [
              {
                id: "0123456789abcdefghijkl",
                name: "John Summit, The Chainsmokers, Ilsey",
              },
            ],
            album: { ...track.album, release_date: "2026-01-01" },
          },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { base: emptyState.playback, track, settings: emptyState.settings },
  );
  await showLyrics(page);
  await emitCanvas(page);
  await page.evaluate(() => document.fonts.ready);
  const overflow = () =>
    page
      .locator(
        ".lyrics-page .lyrics-title .song-title, .lyrics-page .lyrics-artists .artist-link",
      )
      .evaluateAll((elements) => {
        const canvas = document.createElement("canvas").getContext("2d")!;
        return Math.max(
          ...elements.flatMap((element) => {
            const style = getComputedStyle(element);
            canvas.font =
              style.fontStyle +
              " " +
              style.fontWeight +
              " " +
              style.fontSize +
              " " +
              style.fontFamily;
            canvas.letterSpacing =
              style.letterSpacing === "normal" ? "0px" : style.letterSpacing;
            const metrics = canvas.measureText(element.textContent ?? "");
            const marker = document.createElement("span");
            marker.style.cssText =
              "display:inline-block;width:0;height:0;vertical-align:baseline";
            element.prepend(marker);
            const origin = marker.getBoundingClientRect();
            marker.remove();
            const marquee = element.closest(".marquee")!;
            const clip = marquee.getBoundingClientRect();
            return [
              clip.top - (origin.top - metrics.actualBoundingBoxAscent),
              origin.top + metrics.actualBoundingBoxDescent - clip.bottom,
              ...(marquee.classList.contains("scrolling")
                ? []
                : [
                    clip.left - (origin.left - metrics.actualBoundingBoxLeft),
                    origin.left + metrics.actualBoundingBoxRight - clip.right,
                  ]),
            ];
          }),
        );
      });
  await expect.poll(overflow).toBeLessThanOrEqual(1);
  await page.locator(".lyrics-album").screenshot({
    path: "target/diagnostics/lyrics-header-font.png",
    animations: "disabled",
  });
});

test("long song titles fade at both edges while they scroll", async ({
  page,
}) => {
  await startClient(page);
  const name =
    "High Forever - Marlon Hoffstadt Remix (Extended Edition) - Original Mix";
  await page.evaluate(
    ({ base, track, name }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...base, item: { ...track, name } },
        observed_at: Date.now(),
        changed: false,
      }),
    { base: emptyState.playback, track, name },
  );
  const title = page.locator(".now-playing .marquee.scrolling").first();
  await expect(title).toBeVisible();
  const look = await title.evaluate((element) => {
    const style = getComputedStyle(element);
    const content = element.querySelector(".marquee-content")!;
    const text = content.firstElementChild ?? content;
    const parent = element.parentElement!.getBoundingClientRect();
    return {
      mask: style.maskImage || style.webkitMaskImage,
      after: getComputedStyle(element, "::after").content,
      // Text at rest still starts where the panel's content starts.
      start: text.getBoundingClientRect().left - parent.left,
      // Scrolling text never spills past the column into the padding.
      spill: parent.left - element.getBoundingClientRect().left,
    };
  });
  expect(look.mask).toContain("linear-gradient");
  expect(look.after).toBe("none");
  expect(Math.abs(look.start)).toBeLessThanOrEqual(1);
  expect(look.spill).toBeLessThanOrEqual(0);
});

test("the Now playing lyrics card opens, expands and folds like Spicy Lyrics", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await page.getByTitle("Lyrics", { exact: true }).click();
  const card = page.locator(".now-playing .lyric-preview");
  await expect(card).toBeVisible();
  const controls = card.locator(".lyric-card-control");
  await expect(controls).toHaveCount(3);
  await card.screenshot({ path: "target/diagnostics/npv-lyrics-card.png" });
  // The current line sits near the top, not below a band of empty space.
  const lead = await card
    .locator(".lyric-scroll")
    .evaluate((element) =>
      parseFloat(getComputedStyle(element).getPropertyValue("--lead-top")),
    );
  expect(lead).toBeLessThanOrEqual(8);
  // Expand: the card fills the panel and everything else steps aside.
  await card.getByTitle("Expand lyrics").click();
  await expect(page.locator(".now-playing")).toHaveClass(/lyrics-expanded/);
  await expect(page.locator(".now-playing .artist-card")).toBeHidden();
  const panel = (await page.locator(".now-playing").boundingBox())!;
  const expanded = (await card.boundingBox())!;
  expect(expanded.height).toBeGreaterThan(panel.height - 4);
  // Expanded, the song starts a third down, like the lyrics page.
  await expect
    .poll(() =>
      card
        .locator(".lyric-scroll")
        .evaluate(
          (element) =>
            parseFloat(
              getComputedStyle(element).getPropertyValue("--lead-top"),
            ) / element.clientHeight,
        ),
    )
    .toBeGreaterThan(0.25);
  await card.hover();
  await card.getByTitle("Exit expanded lyrics").click();
  await expect(page.locator(".now-playing")).not.toHaveClass(/lyrics-expanded/);
  // Fold: header only.
  await card.getByTitle("Hide lyrics").click();
  await expect(card.locator(".lyrics-display")).toHaveCount(0);
  expect((await card.boundingBox())!.height).toBeLessThan(80);
  await card.getByTitle("Show lyrics").click();
  await expect(card.locator(".lyrics-display")).toBeVisible();
  await card.getByTitle("Open lyrics").click();
  await expect(page.locator(".lyrics-page")).toBeVisible();
});

test("panels use a hover-only overlay scrollbar that takes no space", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 640 });
  await startClient(page);
  const scroller = page.locator(".now-playing-scroll").first();
  const bar = page.locator(".now-playing > .overlay-scrollbar");
  await expect(bar).toBeAttached();
  // No gutter: the native bar is gone.
  expect(
    await scroller.evaluate(
      (element) => (element as HTMLElement).offsetWidth - element.clientWidth,
    ),
  ).toBe(0);
  await page.mouse.move(5, 5);
  await expect(bar).toHaveCSS("opacity", "0");
  await page.locator(".now-playing").hover({ position: { x: 40, y: 200 } });
  await expect(bar).toHaveCSS("opacity", "1");
  const thumb = bar.locator(".overlay-scrollbar-thumb");
  const box = (await thumb.boundingBox())!;
  expect(box.width).toBe(8);
  await page.mouse.move(box.x + 4, box.y + 10);
  await page.mouse.down();
  await page.mouse.move(box.x + 4, box.y + 110, { steps: 5 });
  await page.mouse.up();
  expect(
    await scroller.evaluate((element) => element.scrollTop),
  ).toBeGreaterThan(50);
});

test("clicking empty top bar space never starts a window drag; moving does", async ({
  page,
}) => {
  await startClient(page);
  // Find a point where the top bar itself, not a control, is hit.
  const point = await page.evaluate(() => {
    const bar = document.querySelector(".topbar")!.getBoundingClientRect();
    for (let x = bar.left + 4; x < bar.right; x += 6) {
      const y = bar.top + bar.height / 2;
      if (document.elementFromPoint(x, y)?.classList.contains("topbar"))
        return { x, y };
    }
    return null;
  });
  expect(point).not.toBeNull();
  const drags = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter((entry: any) =>
          String(entry.command).includes("start_dragging"),
        ).length,
    );
  await page.mouse.move(point!.x, point!.y);
  await page.mouse.down();
  await page.mouse.up();
  expect(await drags()).toBe(0);
  await page.mouse.down();
  await page.mouse.move(point!.x + 20, point!.y + 2, { steps: 4 });
  await page.mouse.up();
  await expect.poll(drags).toBe(1);
});

test("library sort menu orders playlists and switches views", async ({
  page,
}) => {
  await startClient(page);
  const playlist = (
    id: string,
    name: string,
    owner: string,
    added_at: number,
  ) => ({
    id,
    name,
    images: [],
    owner: { id: owner, display_name: owner },
    collaborative: false,
    added_at,
  });
  await page.evaluate(
    (playlists) =>
      (window as any).__clientTest.emit({ type: "library", playlists }),
    [
      playlist("pl-b", "Banana", "zed", 300),
      playlist("pl-c", "cherry", "amy", 100),
      playlist("pl-a", "Apple", "mia", 200),
    ],
  );
  const names = () =>
    page
      .locator(".library-list [data-playlist-id^='pl-'] strong")
      .allTextContents();
  const pick = async (name: string) => {
    await page.getByTitle("Sort and view").click();
    await page
      .getByRole("dialog", { name: "Sort and view" })
      .getByRole("button", { name, exact: true })
      .click();
  };
  await pick("Alphabetical");
  await expect.poll(names).toEqual(["Apple", "Banana", "cherry"]);
  await pick("Creator");
  await expect.poll(names).toEqual(["cherry", "Apple", "Banana"]);
  await pick("Recently added");
  await expect.poll(names).toEqual(["Banana", "Apple", "cherry"]);
  // Spotify's own order is the library's order.
  await pick("Custom order");
  await expect.poll(names).toEqual(["Banana", "cherry", "Apple"]);
  await expect(page.getByTitle("Sort and view")).toContainText("Custom order");
  const list = page.locator(".library-list");
  const cover = page.locator("[data-playlist-id='pl-a'] .cover");
  await pick("Compact");
  await expect(list).toHaveClass(/library-view-compact/);
  await expect(cover).toBeHidden();
  await pick("Default grid");
  await expect(list).toHaveClass(/library-view-large_grid/);
  await expect(cover).toBeVisible();
  const box = (await cover.boundingBox())!;
  expect(box.width).toBeGreaterThan(100);
  expect(Math.abs(box.width - box.height)).toBeLessThan(2);
});

test("compact library rows open instead of playing, covers centred", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate((settings) => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "settings",
      settings: { ...settings, compact_library: true },
    });
    host.emit({
      type: "library",
      playlists: [
        {
          id: "compact-playlist",
          name: "Compact playlist",
          images: [],
          owner: { id: "test-user" },
          collaborative: false,
        },
      ],
    });
  }, emptyState.settings);
  const library = page.locator(".library-panel");
  const liked = page.locator(".library-liked");
  const playlist = page.locator('[data-playlist-id="compact-playlist"]');
  // Liked Songs playing must not push its cover off centre.
  await page.evaluate(
    ({ track, playback }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: true,
          context: "spotify:user:test-user:collection",
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { track, playback: emptyState.playback },
  );
  const panel = (await library.boundingBox())!;
  for (const row of [liked, playlist]) {
    await row.hover();
    await expect(row.locator(".library-cover-play")).toHaveCount(0);
    const cover = (await row.locator(".liked-cover, .cover").boundingBox())!;
    expect(cover.width).toBe(48);
    expect(
      Math.abs(cover.x + cover.width / 2 - (panel.x + panel.width / 2)),
    ).toBeLessThan(3);
  }
  // Clicking a cover opens the playlist rather than playing it.
  const requests = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "player",
        ).length,
    );
  const before = await requests();
  await playlist.locator(".cover").click();
  await expect(
    page.locator(".center-panel h1", { hasText: "Compact playlist" }),
  ).toBeVisible();
  expect(await requests()).toBe(before);
  await page
    .locator(".library-panel")
    .screenshot({ path: "target/diagnostics/compact-library.png" });
});

test("custom font lyric fill covers tall letters at every sweep position", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(
    ({ settings, uri }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "settings",
        settings: { ...settings, lyric_font: "hemi head", reduce_motion: true },
      });
      host.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "Test",
          synced: true,
          instrumental: false,
          lines: [
            {
              text: "But please",
              start_ms: 0,
              end_ms: 1000,
              background: false,
              opposite: false,
              words: [
                { text: "But", start_ms: 0, end_ms: 400 },
                { text: " please", start_ms: 400, end_ms: 1000 },
              ],
            },
          ],
        },
      });
      host.emit({
        type: "native_position",
        uri,
        position_ms: 600,
        playing: false,
        observed_at: Date.now(),
      });
    },
    { settings: emptyState.settings, uri: track.uri },
  );
  await page.evaluate(() => document.fonts.ready);
  const line = page.locator(".lyrics-page .lyric-line");
  await expect(line).toHaveCount(1);
  await page.addStyleTag({
    content:
      ".lyric-line { text-shadow:none !important; filter:none !important; transition:none !important } .lyric-syllable { text-shadow:none !important; translate:none !important; scale:none !important }",
  });
  await line.screenshot({
    path: "target/diagnostics/custom-font-fill-before.png",
  });
  // Expanding the paint vertically must not uncover additional glyph pixels.
  const before = await line.screenshot();
  await page.addStyleTag({
    content: ".lyric-paint { padding-block:2em; margin-block:-2em }",
  });
  const expanded = await line.screenshot();
  expect(before.equals(expanded)).toBe(true);
});

test("NPV uses album artwork while Canvas plays on the lyrics page", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.evaluate(
    ({ base, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...base, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: false,
      }),
    { base: emptyState.playback, track },
  );
  await emitCanvas(page);
  await expect(page.locator(".now-playing video")).toBeVisible();
  await showLyrics(page);
  await expect(page.locator(".lyrics-canvas-cover video")).toBeVisible();
  await expect(page.locator(".now-playing video")).toHaveCount(0);
  await expect(page.locator(".now-playing .now-artwork img")).toBeVisible();
  // Pausing leaves the Canvas frame on the lyrics page: neither the NPV nor
  // the lyrics backdrop swaps, on pause or on resume.
  await page.evaluate(() => {
    const w = window as any;
    w.__canvasFlicker = 0;
    new MutationObserver(() => {
      if (
        document.querySelector(".now-playing video") ||
        document.querySelector(".lyrics-page .ambience")
      )
        w.__canvasFlicker++;
    }).observe(document.body, { childList: true, subtree: true });
  });
  for (const is_playing of [false, true, false])
    await page
      .evaluate(
        ({ base, track, is_playing }) =>
          (window as any).__clientTest.emit({
            type: "playback",
            playback: { ...base, item: track, is_playing },
            observed_at: Date.now(),
            changed: false,
          }),
        { base: emptyState.playback, track, is_playing },
      )
      .then(() => page.waitForTimeout(300));
  expect(await page.evaluate(() => (window as any).__canvasFlicker)).toBe(0);
  await expect(page.locator(".now-playing .now-artwork img")).toBeVisible();
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".now-playing video")).toBeVisible();
});

test("Canvas title and release year share the same left edge", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.evaluate(
    ({ base, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: {
            ...track,
            album: { ...track.album, release_date: "2026-01-01" },
          },
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { base: emptyState.playback, track },
  );
  await showLyrics(page);
  await emitCanvas(page);
  const title = (await page
    .locator(".lyrics-title .song-title")
    .boundingBox())!;
  const year = (await page.locator(".lyrics-year").boundingBox())!;
  expect(Math.abs(title.x - year.x)).toBeLessThanOrEqual(1);
});

test("playlist artwork recovers when an inferred Spotify size does not exist", async ({
  page,
}) => {
  await startClient(page);
  const original =
    "https://i.scdn.co/image/ab67706c0000bebb0123456789abcdef01234567";
  await page.route("https://i.scdn.co/image/**", (route) =>
    route.request().url() === original
      ? route.fulfill({
          contentType: "image/svg+xml",
          body: '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="640"><rect width="640" height="640" fill="#9670a7"/></svg>',
        })
      : route.fulfill({ status: 404, body: "Missing variant" }),
  );
  await page.evaluate(
    (url) =>
      (window as any).__clientTest.emit({
        type: "library",
        playlists: [
          {
            id: "cover-playlist",
            name: "Cover playlist",
            images: [{ url }],
            owner: { id: "test-user" },
            collaborative: false,
          },
        ],
      }),
    original,
  );
  const image = page.locator('[data-playlist-id="cover-playlist"] img');
  await expect
    .poll(() =>
      image.evaluate(
        (node: HTMLImageElement) => node.complete && node.naturalWidth > 0,
      ),
    )
    .toBe(true);
});

test("a paused Canvas backdrop survives entering and leaving full screen", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await page.evaluate(
    (base) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...base, is_playing: false },
        observed_at: Date.now(),
        changed: false,
      }),
    { ...emptyState.playback, item: track },
  );
  await emitCanvas(page);
  // The test Canvas is solid purple, so a painted backdrop is not black.
  const painted = () =>
    page.locator(".lyrics-canvas-backdrop").evaluate((element) => {
      const canvas = element as HTMLCanvasElement;
      const [r, g, b] = canvas
        .getContext("2d")!
        .getImageData(canvas.width >> 1, canvas.height >> 1, 1, 1).data;
      return r + g + b > 30;
    });
  await expect.poll(painted).toBe(true);
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await expect.poll(painted).toBe(true);
  await page.keyboard.press("Escape");
  await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  await expect.poll(painted).toBe(true);
});

for (const canvas of [false, true]) {
  test(`fullscreen controls follow ${canvas ? "Canvas dock" : "album artwork"} and disappear on idle`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await startClient(page);
    await showLyrics(page);
    await page.evaluate(() =>
      (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
    );
    if (canvas) await emitCanvas(page);
    await page.keyboard.press("F11");
    await expect(page.locator(".application")).toHaveClass(/immersive/);
    const section = page.locator(".lyrics-page");
    const controls = section.locator(".lyrics-fullscreen-controls");
    const title = section.locator(".lyrics-title");
    await page.mouse.move(900, 300);
    await expect(controls).toHaveCSS("opacity", "1");
    const awakeTitle = (await title.boundingBox())!;
    if (canvas) {
      // Lyrics start just inside the Canvas fade, as outside full screen.
      const video = (await section
        .locator(".lyrics-canvas-cover")
        .boundingBox())!;
      const lyrics = (await section.locator(".lyrics-body").boundingBox())!;
      const fadeEnd = video.x + video.width;
      expect(fadeEnd).toBeLessThanOrEqual(1920 * 0.46);
      expect(lyrics.x).toBeLessThan(fadeEnd);
      expect(lyrics.x).toBeGreaterThan(fadeEnd - 1920 * 0.04);
    }
    if (!canvas) {
      const cover = (await section
        .locator(".lyrics-media > .cover")
        .boundingBox())!;
      const buttons = (await controls
        .locator(".lyrics-fullscreen-buttons")
        .boundingBox())!;
      expect(buttons.y).toBeGreaterThan(cover.y);
      expect(buttons.y + buttons.height).toBeLessThan(cover.y + cover.height);
      // The like heart sits in the middle of the cover.
      const heart = (await controls
        .locator(".lyrics-fullscreen-like")
        .boundingBox())!;
      expect(
        Math.abs(heart.y + heart.height / 2 - (cover.y + cover.height / 2)),
      ).toBeLessThan(4);
      expect(
        Math.abs(heart.x + heart.width / 2 - (cover.x + cover.width / 2)),
      ).toBeLessThan(4);
      const toolbar = (await section.locator(".lyrics-toolbar").boundingBox())!;
      expect(toolbar.y).toBeGreaterThan(cover.y);
      // Both time labels reserve the duration's width, so the seek bar
      // keeps its length while the elapsed time changes.
      const times = controls.locator(".lyrics-fullscreen-times time");
      const elapsed = (await times.first().boundingBox())!.width;
      const total = (await times.last().boundingBox())!.width;
      expect(Math.abs(elapsed - total)).toBeLessThan(0.5);
    } else {
      // The song details sit just above the docked controls.
      const artists = (await section.locator(".lyrics-artists").boundingBox())!;
      const dock = (await controls.boundingBox())!;
      await expect
        .poll(async () => {
          const lifted = (await section
            .locator(".lyrics-artists")
            .boundingBox())!;
          return dock.y - (lifted.y + lifted.height);
        })
        .toBeLessThan(40);
      expect(dock.y).toBeGreaterThan(artists.y);
    }
    // Solid Spotify-style transport icons, not stroked and filled Lucide
    // shapes that blur into thick outlines when scaled up.
    await expect(
      controls.locator(
        ":is(.lucide-rewind, .lucide-fast-forward, .lucide-play, .lucide-pause)",
      ),
    ).toHaveCount(0);
    // Same dark frosted toolbar as outside full screen.
    await expect(section.locator(".lyrics-toolbar")).not.toHaveCSS(
      "background-color",
      "rgba(0, 0, 0, 0)",
    );
    await page.screenshot({
      path: `target/diagnostics/fullscreen-${canvas ? "canvas" : "album"}-awake.png`,
    });
    await expect(controls).toHaveCSS("opacity", "0");
    await expect(controls).toHaveCSS("pointer-events", "none");
    if (canvas)
      await expect
        .poll(async () => (await title.boundingBox())!.y - awakeTitle.y)
        .toBeGreaterThan(80);
    else
      expect(
        Math.abs((await title.boundingBox())!.y - awakeTitle.y),
      ).toBeLessThan(1);
    await page.screenshot({
      path: `target/diagnostics/fullscreen-${canvas ? "canvas" : "album"}-idle.png`,
    });
    await page.keyboard.press("Escape");
    await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  });
}

test("fullscreen seek stays usable through idle and commits once on release", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.keyboard.press("F11");
  const controls = page.locator(".lyrics-fullscreen-controls");
  const seek = controls.getByRole("slider", { name: "Seek position" });
  const bounds = (await seek.boundingBox())!;
  await page.mouse.move(
    bounds.x + bounds.width * 0.5,
    bounds.y + bounds.height / 2,
  );
  await page.mouse.down();
  await page.waitForTimeout(1100);
  await expect(controls).toHaveCSS("opacity", "1");
  await page.mouse.move(
    bounds.x + bounds.width * 0.85,
    bounds.y + bounds.height / 2,
  );
  await page.mouse.up();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands.filter(
            (c: any) =>
              c.command === "player" && c.args.request.command === "seek",
          ).length,
      ),
    )
    .toBe(1);
  // Before playback reports the new spot, the bar stays where it was let go.
  const target = await page.evaluate(
    () =>
      (window as any).__clientTest.commands
        .filter(
          (c: any) =>
            c.command === "player" && c.args.request.command === "seek",
        )
        .at(-1).args.request.positionMs,
  );
  await page.waitForTimeout(300);
  expect(Math.abs(Number(await seek.inputValue()) - target)).toBeLessThan(500);
  await page.keyboard.press("Tab");
  await expect(controls).toHaveCSS("opacity", "1");
});

test("transport buttons brighten without moving and icons sit centred", async ({
  page,
}) => {
  await startClient(page);
  const bar = page.locator("footer.player-bar .transport-buttons");
  const previous = bar.getByTitle("Previous song");
  const before = (await previous.boundingBox())!;
  await previous.hover();
  await expect(previous).toHaveCSS("color", "rgb(255, 255, 255)");
  await expect(previous).toHaveCSS("transform", "none");
  const after = (await previous.boundingBox())!;
  expect(Math.abs(after.width - before.width)).toBeLessThan(0.5);
  // Each glyph's drawn shape is centred in its button: within half a pixel
  // for the symmetric icons, and at most a pixel right for the triangle.
  for (const name of ["Previous song", "Next song"]) {
    const offset = await bar.getByTitle(name).evaluate((button) => {
      const shape = button.querySelector("svg")!;
      const box = shape.getBoundingClientRect(),
        drawn = (shape as SVGSVGElement).getBBox(),
        scale = box.width / 16;
      return drawn.x * scale + (drawn.width * scale) / 2 - box.width / 2;
    });
    expect(Math.abs(offset)).toBeLessThan(0.5);
  }
  const play = await bar.locator(".play-toggle").evaluate((button) => {
    const shape = button.querySelector("svg")!;
    const box = shape.getBoundingClientRect(),
      drawn = (shape as SVGSVGElement).getBBox(),
      scale = box.width / 16,
      outer = button.getBoundingClientRect();
    return (
      box.left +
      drawn.x * scale +
      (drawn.width * scale) / 2 -
      (outer.left + outer.width / 2)
    );
  });
  expect(play).toBeGreaterThanOrEqual(0);
  expect(play).toBeLessThan(1);
  await page.locator("footer.player-bar").screenshot({
    path: "target/diagnostics/transport.png",
  });
});

test("the playing playlist turns green with a speaker and its cover pauses", async ({
  page,
}) => {
  await startClient(page);
  const id = "0123456789abcdefghijkl";
  const playback = (is_playing: boolean) =>
    page.evaluate(
      ({ base, track, id, is_playing }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...base,
            item: track,
            is_playing,
            context: `spotify:playlist:${id}`,
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { base: emptyState.playback, track, id, is_playing },
    );
  await page.evaluate(
    (id) =>
      (window as any).__clientTest.emit({
        type: "library",
        playlists: [
          {
            id,
            name: "Certified Jammers",
            images: [],
            owner: { id: "someone", display_name: "iPixelGalaxy" },
            collaborative: false,
          },
        ],
      }),
    id,
  );
  await playback(true);
  const row = page.locator(`[data-playlist-id="${id}"]`);
  await expect(row).toHaveClass(/playing/);
  await expect(row.locator(".library-item strong")).toHaveCSS(
    "color",
    "rgb(30, 215, 96)",
  );
  await expect(row.getByLabel("Playing")).toBeVisible();
  await row.screenshot({ path: "target/diagnostics/library-playing.png" });
  await row.hover();
  await row.screenshot({
    path: "target/diagnostics/library-playing-hover.png",
  });
  const cover = row.getByRole("button", { name: "Pause Certified Jammers" });
  await cover.click();
  const last = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((c: any) => c.command === "player")
          .at(-1)?.args.request.command,
    );
  expect(await last()).toBe("pause");
  // Paused: still green, no speaker, and the cover resumes.
  await playback(false);
  await expect(row.getByLabel("Playing")).toHaveCount(0);
  await row.getByRole("button", { name: "Play Certified Jammers" }).click();
  expect(await last()).toBe("play");
  await expect(page.locator(".library-liked")).not.toHaveClass(/playing/);
});

test("a green bar names the other device playing and opens the device list", async ({
  page,
}) => {
  await startClient(page);
  const bar = page.locator(".playing-elsewhere");
  await expect(bar).toHaveCount(0);
  const playOn = (name: string) =>
    page.evaluate(
      ({ base, track, name }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...base,
            item: track,
            is_playing: true,
            device: { id: "remote", name, is_active: true },
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { base: emptyState.playback, track, name },
    );
  await playOn("ARASHII-PC");
  await expect(bar).toHaveText("Playing on ARASHII-PC");
  await expect(bar).toHaveCSS("background-color", "rgb(30, 215, 96)");
  await page.locator("footer.player-bar").screenshot({
    path: "target/diagnostics/playing-elsewhere.png",
  });
  // It spans the player's width, 8px in from the window edges.
  const footer = (await page.locator("footer.player-bar").boundingBox())!,
    box = (await bar.boundingBox())!;
  expect(Math.abs(box.x - (footer.x + 8))).toBeLessThan(1.5);
  expect(
    Math.abs(footer.x + footer.width - 8 - (box.x + box.width)),
  ).toBeLessThan(1.5);
  // With Liquid Glass it is the capsule's flush lower edge.
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, liquid_glass: true },
      }),
    emptyState.settings,
  );
  await expect(page.locator(".application")).toHaveClass(/liquid-glass/);
  const glass = (await page.locator("footer.player-bar").boundingBox())!,
    edge = (await bar.boundingBox())!;
  expect(Math.abs(edge.x - glass.x)).toBeLessThan(1.5);
  expect(
    Math.abs(glass.y + glass.height - (edge.y + edge.height)),
  ).toBeLessThan(1.5);
  await page.locator("footer.player-bar").screenshot({
    path: "target/diagnostics/playing-elsewhere-glass.png",
  });
  await bar.click();
  await expect(page.locator(".device-menu")).toBeVisible();
  // This computer playing shows no bar.
  await playOn("Lyssna (test)");
  await expect(bar).toHaveCount(0);
});

test("volume bar turns Spotify green with a knob on hover", async ({
  page,
}) => {
  await startClient(page);
  const volume = page.getByRole("slider", { name: "Volume" });
  await volume.hover();
  await expect(volume).toHaveCSS("background-image", /rgb\(30, 215, 96\)/);
  expect(
    await volume.evaluate(
      (node) => getComputedStyle(node, "::-webkit-slider-thumb").opacity,
    ),
  ).toBe("1");
});

test("fullscreen seek clicks while playing send whole milliseconds", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ track, base }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: true,
          progress_ms: 40_000,
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { track, base: emptyState.playback },
  );
  await showLyrics(page);
  await page.keyboard.press("F11");
  const seek = page
    .locator(".lyrics-fullscreen-controls")
    .getByRole("slider", { name: "Seek position" });
  // A press and release without movement fires no change event, so the
  // seek commits the interpolated song clock, which has fractions.
  for (let i = 0; i < 5; i++) {
    await seek.dispatchEvent("pointerdown", { pointerId: 1, isPrimary: true });
    await seek.dispatchEvent("pointerup", { pointerId: 1, isPrimary: true });
    await page.waitForTimeout(37);
  }
  const positions: number[] = await page.evaluate(() =>
    (window as any).__clientTest.commands
      .filter(
        (c: any) => c.command === "player" && c.args.request.command === "seek",
      )
      .map((c: any) => c.args.request.positionMs),
  );
  expect(positions.length).toBeGreaterThan(0);
  for (const position of positions)
    expect(Number.isInteger(position)).toBe(true);
});

for (const viewportWidth of [1200, 1500, 2000]) {
  test(`Your Library resizes by dragging its edge and collapses like Spotify at ${viewportWidth}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: viewportWidth, height: 900 });
    await startClient(page);
    const library = page.locator(".library-panel");
    const handle = page.locator(".library-resizer");
    const start = (await handle.boundingBox())!;
    const y = start.y + start.height / 2;
    const libraryWidth = async () => (await library.boundingBox())!.width;
    const initialWidth = await libraryWidth();
    await page.mouse.move(start.x + 1, y);
    await page.mouse.down();
    await page.mouse.move(start.x + 120, y, { steps: 6 });
    await page.mouse.up();
    await expect.poll(libraryWidth).toBeGreaterThan(initialWidth + 60);
    // Far left collapses to covers; dragging back out expands it.
    const edge = (await handle.boundingBox())!;
    await page.mouse.move(edge.x + 1, y);
    await page.mouse.down();
    await page.mouse.move(60, y, { steps: 6 });
    await page.mouse.up();
    await expect(library).toHaveClass(/compact-library/);
    await expect(library).toHaveCSS("width", "84px");
    const collapsed = (await handle.boundingBox())!;
    await page.mouse.move(collapsed.x + 1, y);
    await page.mouse.down();
    await page.mouse.move(380, y, { steps: 6 });
    await page.mouse.up();
    await expect(library).not.toHaveClass(/compact-library/);
    await expect.poll(libraryWidth).toBeGreaterThan(300);
    const expandedWidth = await libraryWidth();
    await page.setViewportSize({ width: 1000, height: 900 });
    await expect.poll(libraryWidth).toBe(expandedWidth);
  });
}

test("animator presets import Spicy Lyrics share codes into Custom tuning", async ({
  page,
}) => {
  await startClient(page);
  await expect(page.locator("html")).toHaveAttribute(
    "data-animator",
    "Default",
  );
  await openSettings(page, "Effects");
  const preset = page.getByRole("combobox", { name: "Animator preset" });
  await expect(preset.locator("option")).toHaveText(["Default", "Custom"]);
  await preset.selectOption("Custom");
  await expect(page.locator("html")).toHaveAttribute("data-animator", "Custom");
  await showLyrics(page);
  await openSettings(page, "Effects");
  await page.getByRole("button", { name: "Manage", exact: true }).click();
  // Like the fork, tuning floats beside full screen lyrics.
  const panel = page.getByRole("dialog", { name: "Animator Behavior" });
  await expect(panel).toBeVisible();
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toHaveCount(0);
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await expect(page.locator(".lyrics-page")).toBeVisible();
  // Default sits in the saved list; the prompt shows only until a choice.
  const saved = page.getByRole("combobox", { name: "Saved preset" });
  await expect(saved.locator("option")).toHaveText(["Choose…", "Default"]);
  await page.getByLabel("Word peak scale value", { exact: true }).fill("1.5");
  await page
    .getByLabel("Word peak scale value", { exact: true })
    .press("Enter");
  await saved.selectOption("default");
  await page.getByRole("button", { name: "Load", exact: true }).click();
  await expect(saved.locator("option")).toHaveText(["Default"]);
  await expect(saved).toHaveValue("default");
  await expect(
    page.getByLabel("Word peak scale value", { exact: true }),
  ).toHaveValue("1.075");
  await expect(
    page.getByRole("button", { name: "Delete Current", exact: true }),
  ).toBeDisabled();
  // A code shared from Spicy Lyrics pasted into the name box.
  await page
    .getByLabel("Preset name", { exact: true })
    .fill("SLP1:Bouncy|1.2,1.3,2,1,1,0.88,0.64,1.45,0.2,1.18,0.56");
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await expect(page.locator(".animator-message")).toHaveText(
    "Imported Bouncy.",
  );
  await expect(saved).toHaveValue(/^animator-/);
  await expect(
    page.getByLabel("Word peak scale value", { exact: true }),
  ).toHaveValue("1.2");
  const stored = await page.evaluate(
    () =>
      (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "save_settings")
        .at(-1)?.args.settings.lyrics_animator,
  );
  expect(stored.preset).toBe("Custom");
  expect(stored.saved[0].name).toBe("Bouncy");
  expect(stored.custom.verticalStrength).toBe(2);
  // A code copied elsewhere imports from the clipboard, read natively since
  // the webview refuses reads while it lacks focus.
  await page.evaluate(() => {
    (window as any).__clientTest.clipboard =
      "SLP1:Shared|1.1,1.2,2,1,1,0.88,0.64,1.45,0.2,1.18,0.56";
  });
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await expect(page.locator(".animator-message")).toHaveText(
    "Imported Shared.",
  );
  await expect(page.getByLabel("Preset name", { exact: true })).toHaveCSS(
    "color",
    "rgb(255, 255, 255)",
  );
  await page.screenshot({ path: "target/diagnostics/animator-panel.png" });
  // Closing leaves the full screen Manage entered.
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  // Already in full screen, closing stays there.
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await page.mouse.move(900, 300);
  await page
    .locator(".lyrics-toolbar")
    .getByTitle("Lyssna settings", { exact: true })
    .click();
  await page.getByRole("tab", { name: "Effects", exact: true }).click();
  await page.getByRole("button", { name: "Manage", exact: true }).click();
  await expect(panel).toBeVisible();
  await page.getByRole("button", { name: "Close Animator Behavior" }).click();
  await expect(panel).toHaveCount(0);
  await expect(page.locator(".application")).toHaveClass(/immersive/);
});

test("queued songs show under Next in queue, then Next from the playlist", async ({
  page,
}) => {
  await startClient(page);
  const songs = ["Puppet", "High Forever", "Dracula"].map((name, index) => {
    const id = `queuedsong${index}`.padEnd(22, "x");
    return { ...track, id, uri: `spotify:track:${id}`, name };
  });
  await page.evaluate(
    ({ track, songs, playback }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: true,
          context: "spotify:user:test-user:collection",
        },
        observed_at: Date.now(),
        changed: true,
      });
      host.emit({
        type: "playing_details",
        uri: track.uri,
        details: { artists: track.artists, queue: songs, queued: 1 },
      });
    },
    { track, songs, playback: emptyState.playback },
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const sections = page.locator(".queue-panel .queue-section");
  await expect(sections.locator("h3")).toHaveText([
    "Now playing",
    "Next in queue",
    "Next from: Liked Songs",
  ]);
  await expect(sections.nth(1).locator(".queue-row")).toHaveText(/Puppet/);
  await expect(sections.nth(2).locator(".queue-row")).toHaveCount(2);
  const sent = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "player")
          .at(-1)?.args.request,
    );
  // A queued song can be removed from its menu; playlist songs cannot.
  const menu = page.getByRole("menu", { name: "Song actions" });
  await sections.nth(1).locator(".queue-row").click({ button: "right" });
  await menu.getByRole("menuitem", { name: "Remove from queue" }).click();
  await expect.poll(sent).toMatchObject({
    command: "remove_from_queue",
    uri: songs[0].uri,
  });
  await sections
    .nth(2)
    .locator(".queue-row")
    .first()
    .click({ button: "right" });
  await expect(
    menu.getByRole("menuitem", { name: "Add to queue" }),
  ).toBeVisible();
  await expect(
    menu.getByRole("menuitem", { name: "Remove from queue" }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await sections.nth(1).getByRole("button", { name: "Clear queue" }).click();
  await expect.poll(sent).toMatchObject({ command: "clear_queue" });
});

test("DJ's queue shows only Now playing and songs the listener queued", async ({
  page,
}) => {
  await startClient(page);
  const songs = ["Puppet", "The Red", "The Kill"].map((name, index) => {
    const id = `djqueue${index}`.padEnd(22, "x");
    return { ...track, id, uri: `spotify:track:${id}`, name };
  });
  await page.evaluate(
    ({ track, songs, playback }) => {
      const host = (window as any).__clientTest;
      host.emit({ type: "dj_context", active: true });
      host.emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: true,
          context: "spotify:playlist:37i9dQZF1EYkqdzj48dyYq",
        },
        observed_at: Date.now(),
        changed: true,
      });
      host.emit({
        type: "playing_details",
        uri: track.uri,
        details: { artists: track.artists, queue: songs, queued: 1 },
      });
    },
    { track, songs, playback: emptyState.playback },
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const sections = page.locator(".queue-panel .queue-section");
  await expect(sections.locator("h3")).toHaveText([
    "Now playing",
    "Next in queue",
  ]);
  await expect(page.locator(".queue-panel")).not.toContainText("The Red");
});

test("Start a Jam from a song copies the invite and shows the Jam above the queue", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await startClient(page);
  await page.evaluate(
    ({ track, playback }) => {
      const host = (window as any).__clientTest;
      host.jamToStart = {
        session_id: "jamsession1",
        owner_id: "test-user",
        is_owner: true,
        members: [
          {
            id: "test-user",
            username: "test-user",
            display_name: "Stormanzani",
            image_url: null,
          },
        ],
        invite_url: "https://open.spotify.com/socialsession/jamtoken1",
        qr_svg:
          '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><path d="M0 0h1v1H0zM1 1h1v1H1z"/></svg>',
        queue_only: false,
      };
      host.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
      host.emit({
        type: "playing_details",
        uri: track.uri,
        details: { artists: track.artists, queue: [], queued: 0 },
      });
    },
    { track, playback: emptyState.playback },
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  await page
    .locator(".queue-panel .queue-row")
    .first()
    .click({ button: "right", force: true });
  const menu = page.getByRole("menu", { name: "Song actions" });
  await menu.getByRole("menuitem", { name: "Start a Jam" }).click();
  const jam = page.locator(".queue-panel .jam-card");
  await expect(jam.getByRole("heading")).toHaveText("Stormanzani’s Jam");
  await expect(jam).toContainText("Invite friends to your Jam");
  await expect(jam.locator(".jam-qr")).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("https://open.spotify.com/socialsession/jamtoken1");
  // The Jam sits above Now playing.
  const above = await page.evaluate(
    () =>
      document
        .querySelector(".jam-card")!
        .compareDocumentPosition(document.querySelector(".queue-section")!) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  );
  expect(above).toBeTruthy();
  await jam
    .getByRole("switch", { name: "Let others change what's playing" })
    .click();
  await expect(
    jam.getByRole("switch", { name: "Let others change what's playing" }),
  ).not.toBeChecked();
  await jam.getByRole("button", { name: "Close invite" }).click();
  await expect(jam).not.toContainText("Invite friends to your Jam");
  // While a Jam runs, the song menu invites to it instead.
  await page
    .locator(".queue-panel .queue-row")
    .first()
    .click({ button: "right", force: true });
  await expect(menu.getByRole("menuitem", { name: "Start a Jam" })).toHaveCount(
    0,
  );
  await menu.getByRole("menuitem", { name: "Invite to Jam" }).click();
  await expect(jam).toContainText("Invite friends to your Jam");
  await jam.getByRole("button", { name: "End" }).click();
  await expect(jam).toHaveCount(0);
  // Pasting a Jam link from Spotify joins it.
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "https://spotify.link/IdREaHOTC7b");
    document.body.dispatchEvent(
      new ClipboardEvent("paste", { clipboardData: data, bubbles: true }),
    );
  });
  await expect(jam.getByRole("heading")).toHaveText("Friend’s Jam");
  await expect(jam.getByRole("button", { name: "Leave" })).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "jam")
            .at(-1)?.args,
      ),
    )
    .toMatchObject({
      action: "join",
      link: "https://spotify.link/IdREaHOTC7b",
    });
  await jam.getByRole("button", { name: "Leave" }).click();
  await expect(jam).toHaveCount(0);
  await page
    .locator(".queue-panel .queue-row")
    .first()
    .click({ button: "right", force: true });
  await expect(
    menu.getByRole("menuitem", { name: "Start a Jam" }),
  ).toBeVisible();
});

test("A Jam that ends for this computer leaves the queue when Spotify reports it", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ track, playback }) => {
      const host = (window as any).__clientTest;
      host.jamToStart = {
        session_id: "jamsession1",
        owner_id: "friend",
        is_owner: false,
        members: [
          {
            id: "friend",
            username: "friend",
            display_name: "Friend",
            image_url: null,
          },
        ],
        invite_url: "https://open.spotify.com/socialsession/jamtoken1",
        qr_svg:
          '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><path d="M0 0h1v1H0z"/></svg>',
        queue_only: false,
      };
      host.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
    },
    { track, playback: emptyState.playback },
  );
  await page.evaluate(() => {
    const data = new DataTransfer();
    data.setData(
      "text/plain",
      "https://open.spotify.com/socialsession/jamtoken1",
    );
    document.body.dispatchEvent(
      new ClipboardEvent("paste", { clipboardData: data, bubbles: true }),
    );
  });
  const jam = page.locator(".queue-panel .jam-card");
  await expect(jam.getByRole("heading")).toHaveText("Friend’s Jam");
  // The host ends the Jam: Spotify says so, and the card goes well before
  // the next periodic check.
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.jam = null;
    host.emit({ type: "jam_changed", following: false });
  });
  await expect(jam).toHaveCount(0, { timeout: 2000 });
});

test("a guest leaving drops off the Jam at once, even if a reply lags", async ({
  page,
}) => {
  await startClient(page);
  const member = (id: string, name: string) => ({
    id,
    username: id,
    display_name: name,
    image_url: null,
  });
  const started = {
    session_id: "jamsession2",
    owner_id: "me",
    is_owner: true,
    members: [member("me", "Me"), member("friend", "Friend")],
    invite_url: "https://open.spotify.com/socialsession/jamtoken2",
    qr_svg:
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><path d="M0 0h1v1H0z"/></svg>',
    queue_only: false,
    timestamp: 1000,
  };
  await page.evaluate(
    ({ started, track, playback }) => {
      const host = (window as any).__clientTest;
      host.jam = started;
      host.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
    },
    { started, track, playback: emptyState.playback },
  );
  // A Jam this computer hosts, found when Spotify reports a change.
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "jam_changed",
      following: false,
    }),
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const members = page.locator(".queue-panel .jam-card .jam-members");
  await expect(members).toHaveAttribute("title", "2 in this Jam");
  // Spotify reports the friend left; the Jam service still answers with
  // its older view of the Jam.
  await page.evaluate(
    ({ started, me }) => {
      const host = (window as any).__clientTest;
      host.jam = started;
      host.emit({
        type: "jam_changed",
        following: false,
        jam: { ...started, members: [me], timestamp: 2000 },
      });
    },
    { started, me: member("me", "Me") },
  );
  await expect(members).toHaveAttribute("title", "1 in this Jam");
  await page.waitForTimeout(500);
  await expect(members).toHaveAttribute("title", "1 in this Jam");
});

test("a reshuffled queue never slides rows in from far away", async ({
  page,
}) => {
  await startClient(page);
  const songs = Array.from({ length: 30 }, (_, index) => ({
    ...track,
    uri: `spotify:track:song${String(index).padStart(2, "0")}`,
    name: `Song ${index}`,
  }));
  await page.evaluate(
    ({ track, songs }) =>
      (window as any).__clientTest.emit({
        type: "playing_details",
        uri: track.uri,
        details: { artists: track.artists, queue: songs },
      }),
    { track, songs },
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const panel = page.locator(".queue-panel");
  await expect(panel).toContainText("Song 29");
  // Skip to the next song, then Spotify sends a reshuffled queue.
  const shuffled = songs
    .slice(1)
    .map((song, index) => ({ song, key: (index * 17) % 29 }))
    .sort((a, b) => a.key - b.key)
    .map(({ song }) => song);
  await page.evaluate(
    ({ next, shuffled, playback }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "playback",
        playback: { ...playback, item: next, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
      setTimeout(
        () =>
          host.emit({
            type: "playing_details",
            uri: next.uri,
            details: { artists: next.artists, queue: shuffled },
          }),
        150,
      );
    },
    { next: songs[0], shuffled, playback: emptyState.playback },
  );
  await page.waitForTimeout(210);
  await page.screenshot({ path: "target/diagnostics/queue-reshuffle.png" });
  // Mid-animation, the first rows sit near their places: no blank gap.
  const offsets = await panel
    .locator(".queue-section:last-child .queue-row")
    .evaluateAll((rows) =>
      rows.slice(0, 5).map((row) => {
        const transform = getComputedStyle(row).transform;
        return transform === "none"
          ? 0
          : Math.abs(new DOMMatrix(transform).m42);
      }),
    );
  for (const offset of offsets) expect(offset).toBeLessThan(80);
});

test("Settings reopen on the tab and scroll position they were left at", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  const scroller = page.locator(".lyrics-settings-scroll");
  await scroller.evaluate((node) => node.scrollTo({ top: 300 }));
  await expect
    .poll(() => scroller.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(200);
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "Lyrics", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect
    .poll(() => scroller.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(200);
});

test("lyrics song and artist links dim on hover instead of underlining", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  const title = page.locator(".lyrics-title .song-title");
  await title.hover();
  await expect(title).toHaveCSS("text-decoration-line", "none");
  await expect(title).toHaveCSS("opacity", "0.75");
  const artist = page.locator(".lyrics-artists .artist-link").first();
  await artist.hover();
  await expect(artist).toHaveCSS("text-decoration-line", "none");
  await expect(artist).toHaveCSS("opacity", "0.75");
});

test("lyrics clip only at the page box, not at the text column", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await expect(page.locator(".lyrics-page .lyric-line").first()).toBeVisible();
  const room = await page
    .locator(".lyrics-page .lyric-scroll")
    .evaluate((scroller) => {
      const box = scroller.getBoundingClientRect();
      const text = scroller
        .querySelector(".lyric-line")!
        .getBoundingClientRect();
      return { left: text.left - box.left, right: box.right - text.right };
    });
  // A popping word at either edge has room before anything clips it.
  expect(room.left).toBeGreaterThanOrEqual(48);
  expect(room.right).toBeGreaterThanOrEqual(48);
});

test("bundled translations stay hidden unless turned on", async ({ page }) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Test",
        synced: true,
        instrumental: false,
        lines: [
          {
            text: "Look to the rising sun",
            start_ms: 0,
            end_ms: 2000,
            translation: "迎着初升的朝阳",
            romanization: "",
            background: false,
            opposite: false,
            words: [],
          },
        ],
      },
    });
  }, track.uri);
  const line = page.locator(".lyrics-page .lyric-line").first();
  await expect(line).toContainText("Look to the rising sun");
  await expect(page.locator(".lyric-translation")).toHaveCount(0);
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Show translations" }).check();
  await page.keyboard.press("Escape");
  await expect(page.locator(".lyric-translation")).toHaveText("迎着初升的朝阳");
});

test("Space Gravity over Canvas starts words across the page, not bunched left", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, space_gravity: true },
      }),
    emptyState.settings,
  );
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await emitCanvas(page);
  await expect(page.locator(".lyrics-page")).toHaveClass(/canvas-lyrics/);
  await expect(
    page.locator(".gravity-word[data-spawned]").first(),
  ).toBeVisible();
  await page.waitForTimeout(300);
  const right = await page.locator(".lyrics-page").evaluate((root) => {
    const box = root.getBoundingClientRect();
    const words = [...root.querySelectorAll(".gravity-word[data-spawned]")];
    const centres = words.map((word) => {
      const rect = word.getBoundingClientRect();
      return (rect.left + rect.width / 2 - box.left) / box.width;
    });
    return centres.filter((x) => x > 0.5).length / centres.length;
  });
  // Lines are laid out in the lyrics column on the right before floating.
  expect(right).toBeGreaterThan(0.35);
});

async function gravityWithoutCanvas(page: Page) {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: true,
          progress_ms: 40_000,
          device: { name: "Lyssna", is_active: true },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  await expect(
    page.locator(".gravity-word[data-spawned]").first(),
  ).toBeVisible();
}

test("in a small window the player's extra buttons keep clear of the progress bar", async ({
  page,
}) => {
  await page.setViewportSize({ width: 900, height: 600 });
  await startClient(page);
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: false,
          progress_ms: 0,
          device: { name: "Lyssna", is_active: true },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  const gap = await page.locator(".player-bar").evaluate((bar) => {
    const transport = bar.querySelector(".player-transport")!;
    const right = Math.max(
      ...[...transport.querySelectorAll("*")].map(
        (node) => node.getBoundingClientRect().right,
      ),
    );
    const extras = [...bar.querySelectorAll(".player-extras > *")]
      .map((node) => node.getBoundingClientRect())
      .filter((rect) => rect.width > 0);
    return Math.min(...extras.map((rect) => rect.left)) - right;
  });
  await page.screenshot({ path: "test-results/player-small.png" });
  expect(gap).toBeGreaterThanOrEqual(12);
});

test("credits Show all opens Spotify's grouped Credits dialog", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: track,
          is_playing: false,
          progress_ms: 0,
          device: { name: "Lyssna", is_active: true },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  const card = page.locator(".now-card", { hasText: "Credits" });
  await card.getByRole("button", { name: "Show all" }).click();
  const dialog = page.getByRole("dialog", { name: "Credits" });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator("h2")).toHaveText("Timing test");
  await expect(dialog.locator(".credits-group h3")).toHaveText([
    "Artist",
    "Composition & Lyrics",
    "Production & Engineering",
  ]);
  await expect(dialog).toContainText("Lyricist");
  await expect(dialog).toContainText("Test Label");
  // Main artists can be followed from the dialog, as in Spotify.
  await expect(dialog.getByRole("button", { name: /^Follow/ })).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("lyrics artwork scales with the window and the title keeps to its width", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Canvas in lyrics").uncheck();
  await page.keyboard.press("Escape");
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate(
    ({ track, base }) => {
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: {
            ...track,
            name: "A song title long enough to run well past the artwork",
          },
          is_playing: false,
          progress_ms: 0,
          device: { name: "Lyssna", is_active: true },
        },
        observed_at: Date.now(),
        changed: false,
      });
    },
    { track, base: emptyState.playback },
  );
  const measure = () =>
    page.locator(".lyrics-page").evaluate((root) => {
      const width = (selector: string) =>
        root.querySelector(selector)!.getBoundingClientRect().width;
      return {
        // The most the artwork may take: its column, or half the height.
        room: Math.min(width(".lyrics-album"), innerHeight / 2),
        cover: width(".lyrics-media > .cover"),
        title: width(".lyrics-title"),
        artists: width(".lyrics-artists"),
      };
    });
  const small = await measure();
  // It fills that room rather than stopping at a fixed size.
  expect(small.cover).toBeCloseTo(small.room, 0);
  expect(small.title).toBeLessThanOrEqual(small.cover + 1);
  expect(small.artists).toBeLessThanOrEqual(small.cover + 1);
  await page.screenshot({ path: "test-results/lyrics-art-1600.png" });
  await page.setViewportSize({ width: 2400, height: 1400 });
  await expect
    .poll(async () => (await measure()).cover)
    .toBeGreaterThan(small.cover * 1.2);
  const large = await measure();
  expect(large.cover).toBeCloseTo(large.room, 0);
  expect(large.cover).toBeGreaterThan(460);
  expect(large.title).toBeLessThanOrEqual(large.cover + 1);
  await page.screenshot({ path: "test-results/lyrics-art-2400.png" });
});

test("Space Gravity without Canvas floats around the artwork, not over it", async ({
  page,
}) => {
  await gravityWithoutCanvas(page);
  await page.waitForTimeout(2500);
  const result = await page.locator(".lyrics-page").evaluate((root) => {
    const rect = (selector: string) =>
      root.querySelector(selector)!.getBoundingClientRect();
    const cover = rect(".lyrics-media .cover");
    const title = rect(".lyrics-title");
    const column = rect(".lyrics-body");
    const words = [
      ...root.querySelectorAll(".gravity-word[data-spawned]:not(.exiting)"),
    ].map((word) => word.getBoundingClientRect());
    const centreInside = (word: DOMRect, box: DOMRect) => {
      const x = word.left + word.width / 2,
        y = word.top + word.height / 2;
      return x > box.left && x < box.right && y > box.top && y < box.bottom;
    };
    return {
      count: words.length,
      overCover: words.filter((word) => centreInside(word, cover)).length,
      overTitle: words.filter((word) => centreInside(word, title)).length,
      columnLeft: column.left,
      coverRight: cover.right,
    };
  });
  expect(result.count).toBeGreaterThan(0);
  expect(result.overCover).toBe(0);
  expect(result.overTitle).toBe(0);
  // Credits read as clearly as the pinned footer beside normal lyrics.
  const credits = await page.locator(".gravity-credits").evaluate((node) => ({
    size: parseFloat(getComputedStyle(node.querySelector("p")!).fontSize),
    gap: innerHeight - node.getBoundingClientRect().bottom,
  }));
  expect(credits.size).toBeGreaterThanOrEqual(15);
  // Credits keep their size and place when Space Gravity is turned off.
  const look = (root: string) =>
    page.locator(root).evaluate((node) => {
      const text = (selector: string) => {
        const element = node.querySelector(selector)!;
        return {
          size: parseFloat(getComputedStyle(element).fontSize),
          left: Math.round(element.getBoundingClientRect().left),
        };
      };
      return {
        writers: text(".writer-credits"),
        source: text(".lyric-source"),
        lineHeight: getComputedStyle(node.querySelector(".writer-credits")!)
          .lineHeight,
      };
    });
  const floating = await look(".gravity-credits");
  await openSettings(page, "Effects");
  await page.getByLabel("Space Gravity", { exact: true }).uncheck();
  await page.keyboard.press("Escape");
  const still = await look(".lyric-end-credits");
  expect(floating).toEqual(still);
});

test("Space Gravity keeps words in place when the same lyrics load again", async ({
  page,
}) => {
  await gravityWithoutCanvas(page);
  await page.waitForTimeout(600);
  const places = () =>
    page.locator(".gravity-word[data-spawned]").evaluateAll((words) =>
      Object.fromEntries(
        words.map((word) => {
          const rect = word.getBoundingClientRect();
          return [word.textContent, [rect.left, rect.top]];
        }),
      ),
    );
  const before = await places();
  await page.locator(".file-input").setInputFiles({
    name: "timing.ttml",
    mimeType: "application/xml",
    buffer: Buffer.from("<tt/>"),
  });
  await page.waitForTimeout(120);
  const after = await places();
  const moved = Object.keys(before)
    .filter((key) => after[key])
    .map((key) =>
      Math.hypot(
        after[key][0] - before[key][0],
        after[key][1] - before[key][1],
      ),
    );
  expect(moved.length).toBeGreaterThan(0);
  // Drift over a few frames, not a respawn across the page.
  expect(Math.max(...moved)).toBeLessThan(40);
});

test("clicking a queued song skips to it rather than playing it alone", async ({
  page,
}) => {
  await startClient(page);
  const songs = Array.from({ length: 3 }, (_, index) => ({
    ...track,
    uri: `spotify:track:next${String(index).padStart(2, "0")}`,
    name: `Next ${index}`,
  }));
  await page.evaluate(
    ({ uri, queue, artists }) =>
      (window as any).__clientTest.emit({
        type: "playing_details",
        uri,
        details: { artists, queue },
      }),
    { uri: track.uri, queue: songs, artists: track.artists },
  );
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  await page
    .locator(".queue-panel .queue-row")
    .filter({ hasText: "Next 1" })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "player")
          .map((entry: any) => entry.args.request)
          .at(-1),
      ),
    )
    .toMatchObject({ command: "skip_to", uri: "spotify:track:next01" });
});

test("the queue moves once on skip even when Spotify sends the queue first", async ({
  page,
}) => {
  await startClient(page);
  const songs = Array.from({ length: 12 }, (_, index) => ({
    ...track,
    uri: `spotify:track:next${String(index).padStart(2, "0")}`,
    name: `Next ${index}`,
  }));
  const details = (uri: string, queue: unknown[]) =>
    page.evaluate(
      ({ uri, queue, artists }) =>
        (window as any).__clientTest.emit({
          type: "playing_details",
          uri,
          details: { artists, queue },
        }),
      { uri, queue, artists: track.artists },
    );
  await details(track.uri, songs);
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Queue", exact: true })
    .click();
  const panel = page.locator(".queue-panel");
  const firstNext = panel
    .locator(".queue-section:last-child .queue-row")
    .first();
  await expect(firstNext).toContainText("Next 0");
  await page.waitForTimeout(400);
  // The advanced queue arrives before the song change: nothing moves yet.
  await details(track.uri, songs.slice(1));
  await page.waitForTimeout(50);
  await expect(firstNext).toContainText("Next 0");
  // Then the song changes, and rows slide up from where they show,
  // never dropping back down mid-slide.
  const trace = await page.evaluate(
    async ({ next, playback }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "playback",
        playback: { ...playback, item: next, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
      const tops: number[] = [];
      const row = () =>
        [...document.querySelectorAll(".queue-panel .queue-row")].find(
          (element) => element.textContent?.includes("Next 1"),
        )!;
      await new Promise<void>((resolve) => {
        const start = performance.now();
        const sample = () => {
          tops.push(row().getBoundingClientRect().top);
          if (performance.now() - start < 350) requestAnimationFrame(sample);
          else resolve();
        };
        requestAnimationFrame(sample);
      });
      return tops;
    },
    { next: songs[0], playback: emptyState.playback },
  );
  await expect(
    panel.locator(".queue-section:first-of-type .queue-row"),
  ).toContainText("Next 0");
  await expect(firstNext).toContainText("Next 1");
  for (let index = 1; index < trace.length; index += 1)
    expect(trace[index]).toBeLessThanOrEqual(trace[index - 1] + 0.5);
  expect(trace[0] - trace.at(-1)!).toBeGreaterThan(20);
});

test("the release year dot sits evenly between year and artist", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(
    ({ base, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: {
            ...track,
            album: { ...track.album, release_date: "2025-01-01" },
          },
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { base: emptyState.playback, track },
  );
  const gaps = await page.locator(".lyrics-artists").evaluate((root) => {
    const range = document.createRange();
    const box = (node: Node) => {
      range.selectNodeContents(node);
      return range.getBoundingClientRect();
    };
    const year = box(root.querySelector(".lyrics-year")!.firstChild!);
    const dot = box(root.querySelector(".lyrics-year-dot")!);
    const artist = box(root.querySelector(".lyrics-artists .text-link")!);
    return [dot.left - year.right, artist.left - dot.right];
  });
  expect(Math.abs(gaps[0] - gaps[1])).toBeLessThanOrEqual(2);
});

test("Settings is solid without Liquid Glass and glass with it", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "General");
  const panel = page.locator(".lyrics-settings").first();
  await expect(panel).toHaveCSS("background-color", "rgb(25, 25, 25)");
  await expect(panel).toHaveCSS("backdrop-filter", "none");
  // A clear dim sets Settings apart from the page behind.
  await expect(page.locator(".lyrics-settings-backdrop")).toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0.6)",
  );
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, liquid_glass: true },
      }),
    emptyState.settings,
  );
  await expect(panel).not.toHaveCSS("backdrop-filter", "none");
});

test("full screen artwork keeps its share of the screen at any resolution", async ({
  page,
}) => {
  const share = async (width: number, height: number) => {
    await page.setViewportSize({ width, height });
    await page.waitForTimeout(100);
    const cover = (await page.locator(".lyrics-media > .cover").boundingBox())!;
    return cover.height / height;
  };
  await page.setViewportSize({ width: 1920, height: 1080 });
  await startClient(page);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, canvas_lyrics: false },
      }),
    emptyState.settings,
  );
  await showLyrics(page);
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  const hd = await share(1920, 1080);
  const uhd = await share(3840, 2160);
  expect(hd).toBeGreaterThan(0.4);
  expect(Math.abs(hd - uhd)).toBeLessThan(0.02);
});

test("sung lyrics paint every glyph to its tips, in any font", async ({
  page,
}) => {
  // The fill runs over each syllable's box. Glyphs that reach past their
  // advance (slanted tops, swash tails) must still be painted sung once the
  // syllable is sung, and not sung before it starts.
  const css = readFileSync("frontend/styles.css", "utf8");
  const paint = css.match(/\n\.lyric-paint \{[\s\S]*?\n\}/)![0];
  const fonts = ["Hemi Head", "Gabriola", "Times New Roman"];
  await page.setContent(`<style>
    body { margin: 0; background: #000; }
    ${paint}
    .row { font-size: 110px; padding: 40px 60px; white-space: nowrap; color: transparent; }
    .solid .lyric-paint { background: #fff; -webkit-background-clip: text; background-clip: text; }
    .lyric-paint { --sung-alpha: 1; --unsung-alpha: 0.4; }
  </style>
  ${fonts
    .map((font) =>
      ["solid", "sung", "unsung"]
        .map(
          (kind) =>
            `<div class="row ${kind}" style="font-family: '${font}'; font-style: italic; --gradient-ratio: ${kind === "unsung" ? -0.2 : 1}">${[..."lfjy"].map((letter) => `<span class="lyric-paint">${letter}</span>`).join("")}</div>`,
        )
        .join(""),
    )
    .join("")}`);
  await page.evaluate(() => document.fonts.ready);
  const rows = page.locator(".row");
  const pixels = async (index: number) => {
    const shot = await rows.nth(index).screenshot();
    return page.evaluate(async (data) => {
      const image = new Image();
      image.src = `data:image/png;base64,${data}`;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = image.width;
      canvas.height = image.height;
      const context = canvas.getContext("2d")!;
      context.drawImage(image, 0, 0);
      const values = context.getImageData(0, 0, image.width, image.height).data;
      return Array.from({ length: values.length / 4 }, (_, i) => values[i * 4]);
    }, shot.toString("base64"));
  };
  for (let font = 0; font < fonts.length; font += 1) {
    const [solid, sung, unsung] = await Promise.all(
      [0, 1, 2].map((kind) => pixels(font * 3 + kind)),
    );
    const ink = solid.flatMap((value, index) => (value > 230 ? [index] : []));
    expect(ink.length).toBeGreaterThan(500);
    // Fully inside glyphs: sung is full white, unsung is dim.
    const dimSung = ink.filter((index) => sung[index] < 200).length;
    const brightUnsung = ink.filter((index) => unsung[index] > 140).length;
    expect(dimSung, `${fonts[font]} sung`).toBe(0);
    expect(brightUnsung, `${fonts[font]} unsung`).toBe(0);
  }
});

test("pinned lyric credits line up with the writers above", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, lyrics_pinned_footer: "no_writers" },
      }),
    emptyState.settings,
  );
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  const display = page.locator(".lyrics-page .lyrics-display");
  await display.locator(".lyric-scroll").evaluate((node) => {
    node.scrollTop = node.scrollHeight;
  });
  const lefts = await display.evaluate((root) => {
    const range = document.createRange();
    const start = (node: Element) => {
      range.selectNodeContents(node);
      return range.getClientRects()[0]?.left;
    };
    return {
      writers: start(root.querySelector(".lyric-end-credits p")!),
      source: start(root.querySelector(".lyric-source")!),
    };
  });
  expect(Math.abs(lefts.writers! - lefts.source!)).toBeLessThanOrEqual(1);
});

test("hovering the progress bar previews the time under the pointer", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page, false);
  const bar = page.getByLabel("Seek position");
  const box = (await bar.boundingBox())!;
  await page.mouse.move(box.x + box.width * 0.75, box.y + box.height / 2);
  const tooltip = page.locator(".seek-tooltip");
  await expect(tooltip).toBeVisible();
  await expect(tooltip).toHaveText(/^\d+:\d{2}$/);
  expect(
    await bar.evaluate((node) =>
      parseFloat(node.style.getPropertyValue("--hover")),
    ),
  ).toBeCloseTo(75, 0);
  // The seek bar hovers like the volume bar: green fill, light preview.
  const background = (node: Element) => getComputedStyle(node).backgroundImage;
  const hovered = await bar.evaluate(background);
  expect(hovered).toContain("rgb(30, 215, 96)");
  expect(hovered).toContain("rgba(255, 255, 255, 0.3)");
  const volume = page.getByLabel("Volume", { exact: true });
  const volumeBox = (await volume.boundingBox())!;
  await page.mouse.move(
    volumeBox.x + volumeBox.width * 0.9,
    volumeBox.y + volumeBox.height / 2,
  );
  expect((await volume.evaluate(background)).replace(/[\d.]+%/g, "")).toBe(
    hovered.replace(/[\d.]+%/g, ""),
  );
  await page.mouse.move(box.x + box.width / 2, box.y - 200);
  await expect(tooltip).toHaveCount(0);
  // At rest both end in the same rounded cap.
  expect(await bar.evaluate(background)).toContain("radial-gradient");
});

test("the bell opens What's New with new and earlier releases", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    const release = (name: string, isNew: boolean, daysAgo: number) => ({
      id: `music_release:${name}`,
      uri: `spotify:album:${name.padEnd(22, "x").slice(0, 22)}`,
      name,
      album_type: "SINGLE",
      artists: [
        {
          id: "0pWR7TsFhvSCnbmHDjWgrE",
          uri: "spotify:artist:0pWR7TsFhvSCnbmHDjWgrE",
          name: "Creepy Nuts",
        },
      ],
      images: [{ url: "https://artwork.example.test/cover.svg", width: 640 }],
      released: new Date(Date.now() - daysAgo * 86_400_000).toISOString(),
      new: isNew,
    });
    (window as any).__clientTest.whatsNew = [
      release("Fresh", true, 4),
      release("Older", false, 8),
    ];
  });
  await page.getByRole("button", { name: "What's New" }).click();
  const feed = page.locator(".whats-new-page");
  await expect(feed.getByRole("heading", { name: "What’s New" })).toBeVisible();
  await expect(feed.locator("section").first()).toContainText("New");
  await expect(feed.locator("section").first()).toContainText("Fresh");
  await expect(feed.locator("section").first()).toContainText(
    /Single\s*·\s*4 days ago/,
  );
  await expect(feed.locator("section").nth(1)).toContainText("Earlier");
  await expect(feed.locator("section").nth(1)).toContainText(
    /Single\s*·\s*1 week ago/,
  );
  await page.screenshot({ path: "target/diagnostics/whats-new.png" });
  await feed
    .getByRole("button", { name: "Save to Your Library" })
    .first()
    .click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (entry: any) => entry.command === "save_item" && entry.args.saved,
        ),
      ),
    )
    .toBe(true);
});
test("Your Library lists saved albums and followed artists", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    (window as any).__clientTest.libraryItems = {
      albums: [
        {
          id: "5jGFlFGPRbQYzKUM5ux9v3",
          uri: "spotify:album:5jGFlFGPRbQYzKUM5ux9v3",
          name: "Saved Single",
          images: [],
          artists: [
            { id: "0pWR7TsFhvSCnbmHDjWgrE", uri: "", name: "Creepy Nuts" },
          ],
        },
      ],
      artists: [
        {
          id: "0pWR7TsFhvSCnbmHDjWgrE",
          uri: "spotify:artist:0pWR7TsFhvSCnbmHDjWgrE",
          name: "Creepy Nuts",
          images: [],
        },
      ],
      // Followed just now, saved long ago: Recents puts the artist first.
      recency: {
        "spotify:artist:0pWR7TsFhvSCnbmHDjWgrE": Date.now() - 1000,
        "spotify:album:5jGFlFGPRbQYzKUM5ux9v3": 1,
      },
    };
  });
  const library = page.locator(".library-panel");
  // No Refresh buttons: F5 reloads the library, as in a browser.
  await expect(
    page.getByTitle(/^Refresh (library|page|personalized Home)$/),
  ).toHaveCount(0);
  await page.keyboard.press("F5");
  // No chip is chosen at first: playlists, albums and artists all show.
  await expect(library.locator(".library-filters .chip.selected")).toHaveCount(
    0,
  );
  await expect(
    library.locator(".library-item", { hasText: "Saved Single" }),
  ).toBeVisible();
  await expect(library.locator(".library-liked")).toBeVisible();
  // Mixed by Recents, not grouped by kind.
  const names = await library
    .locator(
      ".library-list > .library-item strong, .library-list > div:not(.library-liked, .library-local) .library-item strong",
    )
    .allTextContents();
  expect(names.indexOf("Creepy Nuts")).toBeLessThan(
    names.indexOf("Saved Single"),
  );
  // Test playlists carry no dates, so both collection rows come first.
  expect(names.slice(0, 2)).toEqual(["Creepy Nuts", "Saved Single"]);
  const playlists = library.getByRole("button", {
    name: "Playlists",
    exact: true,
  });
  await playlists.click();
  await expect(playlists).toHaveAttribute("aria-pressed", "true");
  await expect(
    library.locator(".library-item", { hasText: "Saved Single" }),
  ).toHaveCount(0);
  // Clicking it again goes back to everything.
  await playlists.click();
  await expect(playlists).toHaveAttribute("aria-pressed", "false");
  await expect(
    library.locator(".library-item", { hasText: "Saved Single" }),
  ).toBeVisible();
  await library.getByRole("button", { name: "Albums", exact: true }).click();
  const album = library.locator(".library-item", { hasText: "Saved Single" });
  await expect(album).toContainText("Album · Creepy Nuts");
  await expect(library.locator(".library-liked")).toBeHidden();
  await library.getByRole("button", { name: "Artists", exact: true }).click();
  const artist = library.locator(".library-item", { hasText: "Creepy Nuts" });
  await expect(artist).toContainText("Artist");
  await expect(artist.locator(".cover")).toHaveClass(/artist-cover/);
  await artist.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (entry: any) =>
            entry.command === "browse" &&
            JSON.stringify(entry.args).includes(
              "artist:0pWR7TsFhvSCnbmHDjWgrE",
            ),
        ),
      ),
    )
    .toBe(true);
});

test("What's New waits for the session when the app opens on it", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() => {
    const host = window as any;
    host.__clientTest.emit({ type: "native", ready: false, status: "" });
    host.__clientTest.whatsNew = [
      {
        id: "music_release:late",
        uri: "spotify:album:5jGFlFGPRbQYzKUM5ux9v3",
        name: "Late Release",
        album_type: "SINGLE",
        artists: [],
        images: [],
        released: new Date().toISOString(),
        new: true,
      },
    ];
  });
  await page.getByRole("button", { name: "What's New" }).click();
  const feed = page.locator(".whats-new-page");
  await expect(feed).toContainText("Loading");
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (entry: any) => entry.command === "whats_new",
      ),
    ),
  ).toBe(false);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "native",
      ready: true,
      status: "",
    }),
  );
  await expect(feed).toContainText("Late Release");
});

test("Early scroll is on at 200 ms; its time turns off with it and saves", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  const slider = page.getByLabel("Early scroll time", { exact: true });
  const toggle = page.getByRole("switch", {
    name: "Early scroll",
    exact: true,
  });
  await expect(toggle).toBeChecked();
  await expect(slider).toBeEnabled();
  await expect(slider).toHaveValue("200");
  await toggle.click();
  await expect(slider).toBeDisabled();
  await toggle.click();
  await expect(slider).toBeEnabled();
  await slider.fill("500");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyrics_early_scroll_ms,
      ),
    )
    .toBe(500);
});

test("lyrics show a loading skeleton until they arrive or fail", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const skeleton = page.locator(".lyrics-skeleton");
  await expect(skeleton).toBeVisible();
  await expect(
    page.getByRole("status").filter({ hasText: "Loading lyrics" }),
  ).toBeVisible();
  await expect(skeleton.locator(".lyrics-skeleton-lines span")).toHaveCount(12);
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyric_error",
      uri,
      message: "No lyrics",
    });
  }, track.uri);
  await expect(skeleton).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Lyrics unavailable" }),
  ).toBeVisible();

  await openSettings(page, "Lyrics");
  await page
    .getByRole("switch", { name: "Lyrics loading skeleton", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyrics_skeleton,
      ),
    )
    .toBe(false);
});

test("credits follow a duet that ends on the right when asked", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    const line = (text: string, start: number, opposite: boolean) => ({
      text,
      start_ms: start,
      end_ms: start + 1000,
      translation: "",
      romanization: "",
      background: false,
      opposite,
      words: [],
    });
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        songwriters: ["Test Writer"],
        lines: [line("Left", 1000, false), line("Right", 2000, true)],
      },
    });
  }, track.uri);
  const display = page.locator(".lyrics-display");
  const credits = display.locator(".lyric-end-credits");
  await expect(credits).toHaveCSS("text-align", "start");
  await openSettings(page, "Lyrics");
  await page
    .getByRole("switch", { name: "Credits follow the last singer" })
    .click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(display).toHaveClass(/credits-right/);
  await expect(credits).toHaveCSS("text-align", "right");
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Flip duet sides" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(credits).toHaveCSS("text-align", "start");
});

test("minimal lyrics mode hides sung lines in full screen only", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Minimal lyrics mode" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  const display = page.locator(".lyrics-display");
  await expect(display).toHaveClass(/minimal/);
  const past = display.locator(".lyric-line.past").first();
  const active = display.locator(".lyric-line.active").first();
  const coming = display.locator(".lyric-line:not(.past):not(.active)").last();
  await expect(past).toHaveCSS("opacity", "0.497");
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await expect(past).toHaveCSS("opacity", "0");
  await expect(coming).toHaveCSS("opacity", "0.5");
  await expect(active).toHaveCSS("opacity", "1");
  await page.keyboard.press("Escape");
  await expect(past).toHaveCSS("opacity", "0.497");
});

test("lyrics artwork crossfades to the next song's cover", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const cover = page.locator(".lyrics-media .cover");
  await expect
    .poll(() =>
      cover
        .locator("img")
        .first()
        .evaluate((image) => {
          const element = image as HTMLImageElement;
          return element.complete && element.naturalWidth > 0;
        }),
    )
    .toBe(true);
  await page.evaluate(
    ({ playback, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: {
            ...track,
            uri: "spotify:track:2123456789abcdefghijkl",
            id: "2123456789abcdefghijkl",
            album: {
              ...track.album,
              images: [{ url: "https://artwork.example.test/next.svg" }],
            },
          },
        },
        observed_at: Date.now(),
        changed: true,
      }),
    { playback: emptyState.playback, track },
  );
  await expect(cover.locator(".cover-previous")).toHaveCount(1);
  await expect(cover.locator(".cover-previous")).toHaveCount(0);
  await expect(cover.locator("img")).toHaveAttribute(
    "src",
    "https://artwork.example.test/next.svg",
  );
});

test("full screen can keep the time or controls up while idle", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await openSettings(page, "Layout");
  await page
    .getByLabel("Always show in full screen", { exact: true })
    .selectOption("time");
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  const section = page.locator(".lyrics-page");
  await expect(section).toHaveAttribute("data-always-show", "time");
  const seek = section.locator(".lyrics-glass-seek");
  let buttons = section.locator(".lyrics-media .lyrics-fullscreen-buttons");
  // Idle: the time stays, the buttons hide.
  await page.mouse.move(900, 300);
  await page.mouse.move(901, 301);
  await expect(buttons).toHaveCSS("opacity", "1");
  await expect(buttons).toHaveCSS("opacity", "0", { timeout: 5000 });
  await expect(seek).toHaveCSS("opacity", "1");
  // Kept up, the time leaves the cover for the space under it, above the
  // song's title, as the fork's does; it does not swell on hover.
  const cover = (await section
    .locator(".lyrics-media > .cover")
    .boundingBox())!;
  const seekBox = (await seek.boundingBox())!;
  const title = (await section.locator(".lyrics-title").boundingBox())!;
  expect(seekBox.y).toBeGreaterThan(cover.y + cover.height);
  expect(seekBox.y + seekBox.height).toBeLessThan(title.y);
  await seek.hover();
  await page.waitForTimeout(400);
  expect(
    Math.abs((await seek.boundingBox())!.height - seekBox.height),
  ).toBeLessThan(0.5);
  await page.keyboard.press("Escape");
  await openSettings(page, "Layout");
  await page
    .getByLabel("Always show in full screen", { exact: true })
    .selectOption("controls");
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.keyboard.press("F11");
  await expect(section).toHaveAttribute("data-always-show", "controls");
  // The transport moves under the song details; the heart stays on the
  // cover.
  buttons = section.locator(".outside-transport .lyrics-fullscreen-buttons");
  await expect(seek).toHaveCSS("opacity", "0", { timeout: 5000 });
  await expect(buttons).toHaveCSS("opacity", "1");
  const artists = (await section.locator(".lyrics-artists").boundingBox())!;
  expect((await buttons.boundingBox())!.y).toBeGreaterThan(
    artists.y + artists.height,
  );
  await expect(
    section.locator(".lyrics-media .lyrics-fullscreen-like"),
  ).toHaveCount(1);
  await expect(
    section.locator(".lyrics-media").getByRole("button", { name: "Pause" }),
  ).toHaveCount(0);
});

test("a song without lyrics fills full screen with its cover and controls", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "lyric_error",
        uri,
        message: "No lyrics found for this song",
      }),
    track.uri,
  );
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  const section = page.locator(".lyrics-page");
  await expect(section).toHaveClass(/no-lyrics/);
  await expect(section.locator(".lyrics-message")).toBeHidden();
  await page.mouse.move(800, 300);
  await page.mouse.move(801, 301);
  const cover = (await section
    .locator(".lyrics-media > .cover")
    .boundingBox())!;
  // The cover sits in the middle, the seek bar under it, then the title,
  // then the transport.
  expect(Math.abs(cover.x + cover.width / 2 - 800)).toBeLessThan(4);
  const seek = (await section.locator(".lyrics-glass-seek").boundingBox())!;
  const title = (await section.locator(".lyrics-title").boundingBox())!;
  const transport = (await section
    .locator(".outside-transport")
    .boundingBox())!;
  expect(seek.y).toBeGreaterThan(cover.y + cover.height);
  expect(title.y).toBeGreaterThan(seek.y);
  expect(transport.y).toBeGreaterThan(title.y + title.height);
  await page.screenshot({
    path: "target/diagnostics/fullscreen-no-lyrics.png",
  });
});

test("full screen volume slider sets the volume", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await expect(page.getByLabel("Full screen volume")).toHaveCount(0);
  await page.keyboard.press("Escape");
  for (const side of ["left", "below"]) {
    await openSettings(page, "Layout");
    await page
      .getByLabel("Full screen volume slider", { exact: true })
      .selectOption(side);
    await page.getByRole("button", { name: "Close settings" }).click();
    await page.keyboard.press("F11");
    await page.mouse.move(900, 300);
    await page.mouse.move(901, 302);
    await page.keyboard.press("Escape");
  }
  await emitCanvas(page);
  await page.keyboard.press("F11");
  await page.mouse.move(900, 300);
  await page.mouse.move(901, 302);
  const slider = page.getByLabel("Full screen volume");
  await expect(slider).toBeVisible();
  await slider.fill("40");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .map((entry: any) => entry.args?.request)
            .filter((request: any) => request?.command === "volume")
            .at(-1)?.volume,
      ),
    )
    .toBe(40);
});

test("full screen volume is a pill beside the cover that widens in use", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await openSettings(page, "Layout");
  await page
    .getByLabel("Full screen volume slider", { exact: true })
    .selectOption("right");
  await page.getByRole("button", { name: "Close settings" }).click();
  for (const canvas of [false, true]) {
    if (canvas) await emitCanvas(page);
    await page.keyboard.press("F11");
    await page.mouse.move(900, 300);
    await page.mouse.move(901, 302);
    const pill = page.locator(".lyrics-volume-pill");
    await expect(pill).toBeVisible();
    if (!canvas) {
      // Beside the cover's right edge, about as tall as it.
      const cover = (await page
        .locator(".lyrics-media > .cover")
        .boundingBox())!;
      const box = (await pill.boundingBox())!;
      expect(box.x).toBeGreaterThan(cover.x + cover.width);
      expect(box.height).toBeGreaterThan(cover.height * 0.8);
    } else {
      // Beside a Canvas it lies flat under the seek bar, right of the
      // transport, which starts at the bottom-left.
      const seek = (await page.locator(".lyrics-glass-seek").boundingBox())!;
      const buttons = (await page
        .locator(".lyrics-fullscreen-buttons > button")
        .last()
        .boundingBox())!;
      const like = (await page
        .locator(".lyrics-fullscreen-like")
        .boundingBox())!;
      const box = (await pill.boundingBox())!;
      expect(box.width).toBeGreaterThan(box.height * 3);
      expect(box.y).toBeGreaterThan(seek.y + seek.height);
      expect(box.x).toBeGreaterThan(buttons.x + buttons.width);
      expect(Math.abs(like.x - seek.x)).toBeLessThan(4);
    }
    const axis = canvas ? "height" : "width";
    const before = (await pill.boundingBox())![axis];
    await pill.hover();
    await expect
      .poll(async () => (await pill.boundingBox())![axis])
      .toBeGreaterThan(before + 4);
    await page.screenshot({
      path: `target/diagnostics/fullscreen-volume-${canvas ? "canvas" : "album"}.png`,
    });
    // Its speaker mutes.
    await page.getByRole("button", { name: "Mute" }).click();
    await expect(pill).toHaveAttribute("data-level", "muted");
    await page.getByRole("button", { name: "Unmute" }).click();
    await page.keyboard.press("Escape");
  }
});

test("closing full screen can slide the lyrics away first", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Layout");
  await page
    .getByRole("switch", { name: "Animate closing full screen" })
    .click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await page.keyboard.press("Escape");
  const section = page.locator(".lyrics-page");
  await expect(section).toHaveClass(/fullscreen-exit/);
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  await expect(section).not.toHaveClass(/fullscreen-exit/);
});

test("lyrics view controls can sit at the bottom", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await openSettings(page, "Layout");
  await page
    .getByLabel("View controls position", { exact: true })
    .selectOption("bottom");
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  const section = page.locator(".lyrics-page");
  const toolbar = section.locator("> .lyrics-toolbar");
  await page.mouse.move(900, 400);
  const sectionBox = (await section.boundingBox())!;
  const box = (await toolbar.boundingBox())!;
  expect(sectionBox.y + sectionBox.height - (box.y + box.height)).toBeLessThan(
    30,
  );
  expect(sectionBox.x + sectionBox.width - (box.x + box.width)).toBeLessThan(
    30,
  );
  await emitCanvas(page);
  await page.mouse.move(901, 401);
  const canvasBox = (await toolbar.boundingBox())!;
  expect(
    sectionBox.y + sectionBox.height - (canvasBox.y + canvasBox.height),
  ).toBeLessThan(30);
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await page.mouse.move(900, 400);
  const fullBox = (await section.locator(".lyrics-toolbar").boundingBox())!;
  expect(fullBox.y).toBeLessThan(100);
});

test("the return-to-lyric button can be turned off", async ({ page }) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  const display = page.locator(".lyrics-display");
  const back = display.getByRole("button", { name: "Return to current lyric" });
  await display.locator(".lyric-scroll").hover();
  await page.mouse.wheel(0, 1400);
  await expect(display).toHaveAttribute("data-following", "false");
  await expect(back).toBeVisible();
  await openSettings(page, "Lyrics");
  await page
    .getByRole("switch", { name: "Show scroll to active button" })
    .click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(display).toHaveAttribute("data-following", "false");
  await expect(back).toHaveCount(0);
});

test("Now playing's dynamic background can be turned off", async ({ page }) => {
  await startClient(page);
  const panel = page.locator("aside.now-playing");
  await expect(panel.locator(".ambience")).toHaveCount(1);
  await openSettings(page, "Effects");
  await page
    .getByRole("switch", { name: "Dynamic background in Now playing" })
    .click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(panel.locator(".ambience")).toHaveCount(0);
});

test("community profiles can open in the browser", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Open profiles in browser" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.getByTitle("Lyrics", { exact: true }).click();
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "spicy_lyrics",
        synced: true,
        instrumental: false,
        attribution: {
          maker: {
            username: "Maker",
            id: "123456789012345678",
            url: "https://spicylyrics.org/profile/Maker",
          },
        },
        lines: [
          {
            text: "Hello",
            start_ms: 1000,
            end_ms: 2000,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: [],
          },
        ],
      },
    });
  }, track.uri);
  await page.getByRole("button", { name: "@Maker", exact: true }).click();
  const calls = () =>
    page.evaluate(() =>
      (window as any).__clientTest.commands
        .filter((call: any) =>
          ["open_link", "open_creator_profile"].includes(call.command),
        )
        .map((call: any) => [call.command, call.args?.url ?? null]),
    );
  await expect
    .poll(calls)
    .toEqual([["open_link", "https://spicylyrics.org/profile/Maker"]]);
});

test("Escape can leave the lyrics page as well as full screen", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.keyboard.press("Escape");
  await expect(page.locator(".lyrics-page")).toBeVisible();
  await openSettings(page, "Layout");
  await page
    .getByLabel("Escape key function", { exact: true })
    .selectOption("fully");
  // Escape closes Settings first without leaving the lyrics.
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toHaveCount(0);
  await expect(page.locator(".lyrics-page")).toBeVisible();
  await page.keyboard.press("F11");
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  await page.keyboard.press("Escape");
  await expect(page.locator(".application")).not.toHaveClass(/immersive/);
  await expect(page.locator(".lyrics-page")).toHaveCount(0);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect(page.locator(".lyrics-page")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".lyrics-page")).toHaveCount(0);
});

test("release year can sit after the artists or be left out", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(
    ({ base, track }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...base,
          item: {
            ...track,
            album: { ...track.album, release_date: "2026-01-01" },
          },
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { base: emptyState.playback, track },
  );
  await showLyrics(page);
  const artists = page.locator(".lyrics-page .lyrics-artists");
  const year = artists.locator(".lyrics-year");
  await expect(year).toHaveCount(1);
  await expect(artists.locator(":scope > *").first()).toHaveClass(
    /lyrics-year/,
  );
  await openSettings(page, "Lyrics");
  const select = page.getByLabel("Release year position", { exact: true });
  await select.selectOption("right");
  await expect(artists.locator(":scope > *").last()).toHaveClass(
    /lyrics-year right/,
  );
  await select.selectOption("off");
  await expect(year).toHaveCount(0);
});

test("the lyrics cache button follows its chosen action", async ({ page }) => {
  await startClient(page);
  await showLyrics(page);
  const calls = () =>
    page.evaluate(() =>
      (window as any).__clientTest.commands
        .filter((call: any) =>
          ["refresh_lyrics", "clear_lyrics_cache"].includes(call.command),
        )
        .map((call: any) => [call.command, call.args?.keepSaved ?? null]),
    );
  const toolbar = page.locator(".lyrics-toolbar");
  await page.mouse.move(700, 400);
  await toolbar.getByTitle(/clears this song's saved copy/).click();
  await openSettings(page, "Sources");
  await page
    .getByLabel("Lyrics cache button action", { exact: true })
    .selectOption("reload");
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.mouse.move(701, 401);
  await toolbar.getByTitle(/keeps saved copies/).click();
  await openSettings(page, "Sources");
  await page
    .getByLabel("Lyrics cache button action", { exact: true })
    .selectOption("all");
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.mouse.move(702, 402);
  await toolbar.getByTitle(/clears every saved copy/).click();
  await expect.poll(calls).toEqual([
    ["refresh_lyrics", false],
    ["refresh_lyrics", true],
    ["clear_lyrics_cache", null],
  ]);
  await openSettings(page, "Sources");
  await page.getByRole("switch", { name: "Lyrics cache button" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(toolbar.locator('[data-icon="retry"]')).toHaveCount(0);
});

test("Apple Music quality priority is on until turned off", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Sources");
  const toggle = page.getByRole("switch", {
    name: "Prioritize Apple Music quality",
  });
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyrics_prefer_apple,
      ),
    )
    .toBe(false);
});

test("Musixmatch word sync can be ignored", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Sources");
  const toggle = page.getByRole("switch", {
    name: "Ignore Musixmatch word sync",
  });
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyrics_musixmatch_lines_only,
      ),
    )
    .toBe(true);
});

test("a Musixmatch token is saved without being shown again", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Sources");
  const field = page.getByLabel("Musixmatch token", { exact: true });
  await page.getByRole("button", { name: "Refresh token" }).click();
  await expect(page.locator(".toast")).toContainText(
    "Musixmatch token refreshed",
  );
  await field.fill("abcdefghij0123456789abcd");
  await page.getByRole("button", { name: "Save token" }).click();
  await expect(field).toHaveValue("");
  await expect(field).toHaveAttribute("placeholder", "Saved");
  await expect(page.getByRole("button", { name: "Refresh token" })).toHaveCount(
    0,
  );
  await page.getByRole("button", { name: "Remove token" }).click();
  await expect(
    page.getByRole("button", { name: "Refresh token" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands
          .filter((call: any) => call.command === "save_musixmatch_token")
          .map((call: any) => call.args.token),
      ),
    )
    .toEqual(["abcdefghij0123456789abcd", ""]);
});

test("Settings can forget where it was closed", async ({ page }) => {
  await startClient(page);
  const dialog = page.getByRole("dialog", { name: "Lyssna settings" });
  await openSettings(page, "Advanced");
  await page
    .getByRole("switch", { name: "Remember settings location" })
    .click();
  await page.getByRole("tab", { name: "Lyrics", exact: true }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.getByRole("button", { name: "Account menu", exact: true }).click();
  await page.getByRole("menuitem", { name: "Settings", exact: true }).click();
  await expect(
    dialog.getByRole("tab", { name: "General", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("developer tools open only in Developer mode", async ({ page }) => {
  await startClient(page);
  const opened = () =>
    page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (call: any) => call.command === "open_devtools",
        ).length,
    );
  await page.keyboard.press("F12");
  expect(await opened()).toBe(0);
  await openSettings(page, "Advanced");
  await expect(page.getByText("Developer tools", { exact: true })).toHaveCount(
    0,
  );
  await page.getByRole("switch", { name: "Developer mode" }).click();
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await expect.poll(opened).toBe(1);
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.keyboard.press("F12");
  await expect.poll(opened).toBe(2);
});

test("the full screen seek bar can drop its glass styling", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.keyboard.press("F11");
  const seek = page.locator(".lyrics-glass-seek");
  await expect(seek).not.toHaveCSS("backdrop-filter", "none");
  await page.keyboard.press("Escape");
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Glass seek bar" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.keyboard.press("F11");
  await expect(seek).toHaveCSS("backdrop-filter", "none");
});

test("unique word filters change the case of every lyric", async ({ page }) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  const line = page.locator(".lyrics-display .lyric-line").first();
  await expect(line).toContainText("Test lyric line 1");
  await openSettings(page, "Effects");
  await page
    .getByLabel("Unique word filters", { exact: true })
    .selectOption("upper");
  await expect(line).toContainText("TEST LYRIC LINE 1");
  await page
    .getByLabel("Unique word filters", { exact: true })
    .selectOption("lower");
  await expect(line).toContainText("test lyric line 1");
});

test("minimal mode keeps overlapping lines dimmed unless told not to", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Minimal lyrics mode" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  await page.evaluate((uri) => {
    const line = (text: string, start: number, end: number) => ({
      text,
      start_ms: start,
      end_ms: end,
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
      words: [],
    });
    const host = (window as any).__clientTest;
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          line("Long lead", 1000, 20000),
          line("Short overlap", 2000, 3000),
          line("Later", 30000, 31000),
        ],
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 5000,
      playing: false,
      observed_at: Date.now(),
    });
  }, track.uri);
  await page.keyboard.press("F11");
  const overlap = page.locator(".lyric-line", { hasText: "Short overlap" });
  await expect(overlap).toHaveClass(/past/);
  await expect(overlap).toHaveCSS("opacity", "0.45");
  await page.keyboard.press("Escape");
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Keep overlapping lines" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await page.keyboard.press("F11");
  await expect(overlap).toHaveCSS("opacity", "0");
});

test("lyrics still follow the song with smooth scrolling off", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Smooth scrolling" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await page.evaluate(
    (uri) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 120_500,
        playing: false,
        observed_at: Date.now(),
      }),
    track.uri,
  );
  const flow = page.locator(".lyrics-display .lyric-scroll");
  await expect
    .poll(() =>
      flow.evaluate((node) => {
        const box = node.getBoundingClientRect(),
          line = node
            .querySelector(".lyric-line.active")!
            .getBoundingClientRect();
        return Math.abs(
          line.top + line.height / 2 - (box.top + box.height / 2),
        );
      }),
    )
    .toBeLessThan(40);
});

test("songs without lyrics can show only the song", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const fail = (message: string) =>
    page.evaluate(
      ({ uri, message }) =>
        (window as any).__clientTest.emit({
          type: "lyric_error",
          uri,
          message,
        }),
      { uri: track.uri, message },
    );
  await fail("No lyrics found for this song");
  const heading = page.getByRole("heading", { name: "Lyrics unavailable" });
  await expect(heading).toBeVisible();
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "No lyrics notice" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(heading).toHaveCount(0);
  await expect(page.locator(".lyrics-page .lyrics-title")).toContainText(
    "Timing test",
  );
  // A source that cannot be reached still says so.
  await fail("Spicy Lyrics is unreachable");
  await expect(heading).toBeVisible();
});

test("lyrics can sit over the cover, the artist's header or nothing", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  const section = page.locator(".lyrics-page");
  const image = section.locator(".lyrics-image-background");
  await expect(section.locator(".ambience")).toHaveCount(1);
  await openSettings(page, "Lyrics");
  const select = page.getByLabel("Lyrics background", { exact: true });
  const blur = page.getByLabel("Background blur", { exact: true });
  await expect(blur).toBeDisabled();
  await select.selectOption("cover");
  await blur.fill("20");
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(section.locator(".ambience")).toHaveCount(0);
  await expect(image).toHaveCSS("background-image", /cover\.svg/);
  await expect(image).toHaveCSS("filter", /blur\(20px\)/);
  await page.evaluate(
    ({ uri, artist }) =>
      (window as any).__clientTest.emit({
        type: "playing_details",
        uri,
        details: {
          artists: [
            {
              ...artist,
              header_image: "https://artwork.example.test/artist/banner.svg",
            },
          ],
          queue: [],
        },
      }),
    { uri: track.uri, artist: track.artists[0] },
  );
  await openSettings(page, "Lyrics");
  await select.selectOption("artist");
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(image).toHaveCSS("background-image", /banner\.svg/);
  await openSettings(page, "Lyrics");
  await select.selectOption("none");
  await page.getByRole("button", { name: "Close settings" }).click();
  await expect(image).toHaveCount(0);
  await expect(section.locator(".ambience")).toHaveCount(0);
});

test("animation frame rate can be capped", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Appearance");
  const rate = page.getByLabel("Animation frame rate", { exact: true });
  await expect(rate).toBeDisabled();
  await page
    .getByRole("switch", { name: "Limit animation frame rate" })
    .click();
  await rate.fill("30");
  await expect
    .poll(() =>
      page.evaluate(() => {
        const settings = (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "save_settings")
          .at(-1)?.args.settings;
        return [
          settings?.animation_fps_cap_enabled,
          settings?.animation_fps_cap,
        ];
      }),
    )
    .toEqual([true, 30]);
});

test("settings can be hidden and shown again", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  const row = page.getByRole("switch", { name: "Show romanized lyrics" });
  await expect(row).toBeVisible();
  await expect(page.getByTitle("Hide Show romanized lyrics")).toHaveCount(0);
  await openSettings(page, "Advanced");
  await page.getByRole("switch", { name: "Allow hiding settings" }).click();
  await expect(page.getByText("No hidden settings")).toBeVisible();
  await openSettings(page, "Lyrics");
  await page.getByTitle("Hide Show romanized lyrics").click();
  await expect(row).toHaveCount(0);
  await openSettings(page, "Advanced");
  const hidden = page
    .locator(".setting-row")
    .filter({ hasText: "Show romanized lyrics" });
  await expect(hidden).toBeVisible();
  // Hide buttons can go while hidden settings stay hidden.
  await page.getByRole("switch", { name: "Show hide buttons" }).click();
  await openSettings(page, "Lyrics");
  await expect(row).toHaveCount(0);
  await expect(page.locator(".setting-hide")).toHaveCount(0);
  await openSettings(page, "Advanced");
  await hidden.getByRole("button", { name: "Show" }).click();
  await openSettings(page, "Lyrics");
  await expect(row).toBeVisible();
});

test("every settings tab keeps its icon and label", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Appearance");
  // Narrow windows drop the icons so all eight labels still fit.
  for (const [width, icons] of [
    [700, false],
    [1024, true],
    [1920, true],
  ] as const) {
    await page.setViewportSize({ width, height: 900 });
    const tabs = await page
      .locator(".lyrics-settings-tabs button")
      .evaluateAll((buttons) =>
        buttons.map((button) => ({
          label: button.textContent,
          icon: button.querySelector("svg")?.getBoundingClientRect().width,
          clipped: button.scrollWidth - button.clientWidth,
        })),
      );
    for (const tab of tabs) {
      if (icons)
        expect(tab.icon, `${tab.label} icon at ${width}px`).toBeGreaterThan(12);
      else expect(tab.icon ?? 0, `${tab.label} icon at ${width}px`).toBe(0);
      expect(tab.clipped, `${tab.label} label at ${width}px`).toBeLessThan(1);
    }
  }
});

test("settings slider values stay inside their rows", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Lyrics");
  const offset = page.getByRole("slider", { name: "Timing offset" });
  await offset.fill("-2000");
  await expect(
    page.locator(".setting-row").filter({ has: offset }).locator("output"),
  ).toHaveText("-2000 ms");
  await openSettings(page, "Appearance");
  await page
    .getByRole("switch", { name: "Limit animation frame rate" })
    .click();
  await page.getByRole("slider", { name: "Animation frame rate" }).fill("240");
  for (const tab of ["Appearance", "Lyrics", "Effects"]) {
    await openSettings(page, tab);
    const overflow = await page
      .locator(".lyrics-settings .settings-range")
      .evaluateAll((ranges) =>
        ranges.map((range) => {
          const row = range.closest(".setting-row")!.getBoundingClientRect();
          const output = range.querySelector("output")!;
          // The text itself, which can spill past a too-narrow output box.
          const text = document.createRange();
          text.selectNodeContents(output);
          return {
            text: output.textContent,
            past: text.getBoundingClientRect().right - (row.right - 16),
          };
        }),
      );
    for (const { text, past } of overflow)
      expect(past, `${text} on ${tab}`).toBeLessThanOrEqual(0.5);
  }
});

test("gibberish respells lyrics and long phrases still wrap", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page
    .getByLabel("Unique word filters", { exact: true })
    .selectOption("gibberish");
  await showLyrics(page);
  const sentence =
    "We know me chain so much you are beautiful and I will be there forever and always";
  await page.evaluate(
    ({ track, playback, sentence }) => {
      const { emit } = (window as any).__clientTest;
      emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: true,
          progress_ms: 0,
        },
        observed_at: Date.now(),
        changed: false,
      });
      const words = sentence.split(" ");
      emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [0, 1].map((line) => ({
            start_ms: line * 20_000,
            end_ms: (line + 1) * 20_000,
            text: sentence,
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: words.map((word, index) => ({
              start_ms: line * 20_000 + index * 500,
              end_ms: line * 20_000 + (index + 1) * 500,
              text: index < words.length - 1 ? `${word} ` : word,
            })),
          })),
        },
      });
    },
    { track, playback: emptyState.playback, sentence },
  );
  const line = page.locator(".lyrics-page .lyric-line").first();
  await expect(line).toContainText("winumichen");
  await expect(line).not.toContainText("beautiful");
  for (const width of [1000, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await expect
      .poll(() =>
        line.evaluate((element) => element.scrollWidth - element.clientWidth),
      )
      .toBeLessThanOrEqual(2);
  }
});

test("highlighter inks sung words behind the text and keeps them", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await expect(page.getByLabel("Marker colour", { exact: true })).toHaveValue(
    "#d4b80f",
  );
  await showLyrics(page);
  await page.evaluate(
    ({ track, playback }) => {
      const { emit } = (window as any).__clientTest;
      emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: false,
          progress_ms: 1500,
        },
        observed_at: Date.now(),
        changed: false,
      });
      const words =
        "As he came into the window was a sound of a crescendo".split(" ");
      emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [0, 1].map((line) => ({
            start_ms: line * 6000,
            end_ms: (line + 1) * 6000,
            text: words.join(" "),
            translation: "",
            romanization: "",
            background: false,
            opposite: false,
            words: words.map((word, index) => ({
              start_ms: line * 6000 + index * 500,
              end_ms: line * 6000 + (index + 1) * 500,
              text: index < words.length - 1 ? `${word} ` : word,
            })),
          })),
        },
      });
    },
    { track, playback: emptyState.playback },
  );
  const active = page.locator(".lyrics-page .lyric-line.active");
  await expect(active.locator(".highlighter-piece")).toHaveCount(12);
  const inks = () =>
    active
      .locator(".highlighter-piece")
      .evaluateAll((pieces) =>
        pieces.map((piece) =>
          Number((piece as HTMLElement).style.getPropertyValue("--ink")),
        ),
      );
  // At 1.5 s the first three words are sung and the rest are not.
  await expect.poll(async () => (await inks()).slice(0, 3)).toEqual([1, 1, 1]);
  expect((await inks()).slice(4)).toEqual(Array(8).fill(0));
  // One stroke on the first row, its pen tip where "came " ends, behind
  // the text.
  const strokes = active.locator(".highlighter-stroke");
  await expect(strokes).toHaveCount(1);
  const reach = () =>
    active.evaluate((line) => {
      const stroke = line.querySelector(".highlighter-stroke")!;
      const tip = line.querySelector(".highlighter-tip")!;
      const came = line.querySelectorAll(".highlighter-piece")[2];
      return {
        gap: Math.abs(
          tip.getBoundingClientRect().right -
            came.getBoundingClientRect().right,
        ),
        behind:
          Number(getComputedStyle(stroke.parentElement!).zIndex) <
          Number(getComputedStyle(came).zIndex),
        mask: (stroke as HTMLElement).style.maskImage,
      };
    });
  await expect.poll(async () => (await reach()).gap).toBeLessThan(14);
  expect((await reach()).behind).toBe(true);
  expect((await reach()).mask).toContain("linear-gradient");
  // Lines yet to be sung carry no strokes.
  await expect(
    page.locator(".lyrics-page .lyric-line:not(.active) .highlighter-stroke"),
  ).toHaveCount(0);
  // Once sung, a line keeps its strokes over every row.
  await page.evaluate(
    ({ track, playback }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: false,
          progress_ms: 7000,
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { track, playback: emptyState.playback },
  );
  const sung = page.locator(".lyrics-page .lyric-line").first();
  await expect(sung).not.toHaveClass(/active/);
  const rows = await sung.evaluate(
    (line) =>
      new Set(
        [...line.querySelectorAll(".highlighter-piece")].map(
          (piece) => (piece as HTMLElement).offsetTop,
        ),
      ).size,
  );
  await expect(sung.locator(".highlighter-stroke")).toHaveCount(rows);
  // One pen tip per line, where the pen finished, not one per row.
  await expect(sung.locator(".highlighter-tip:not(.lifted)")).toHaveCount(1);
  // That tip sits after the last word: its middle is past the last letter.
  await expect
    .poll(() =>
      sung.evaluate((line) => {
        const tip = line.querySelector(".highlighter-tip:not(.lifted)")!;
        const pieces = line.querySelectorAll(".highlighter-piece");
        const last = pieces[pieces.length - 1];
        const range = document.createRange();
        range.selectNodeContents(last);
        return (
          (tip.getBoundingClientRect().left +
            tip.getBoundingClientRect().right) /
            2 -
          range.getBoundingClientRect().right
        );
      }),
    )
    .toBeGreaterThan(0);
  await openSettings(page, "Effects");
  await page
    .getByLabel("Marker colour", { exact: true })
    .selectOption("#3f8ee8");
  await showLyrics(page);
  expect(
    await sung
      .locator(".highlighter-stroke")
      .first()
      .evaluate((stroke) => getComputedStyle(stroke).backgroundColor),
  ).toBe("rgb(63, 142, 232)");
});

test("highlighter ink darkens when slow and marks a pause", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await showLyrics(page);
  await page.evaluate(
    ({ track, playback }) => {
      const { emit } = (window as any).__clientTest;
      emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: false,
          progress_ms: 4200,
        },
        observed_at: Date.now(),
        changed: false,
      });
      // Quick words, a pause after "was", then one long held word.
      const timing: [string, number, number][] = [
        ["Quick ", 0, 150],
        ["words ", 150, 300],
        ["was ", 300, 450],
        ["looong", 1650, 4000],
      ];
      emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              start_ms: 0,
              end_ms: 4000,
              text: "Quick words was looong",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: timing.map(([text, start_ms, end_ms]) => ({
                text,
                start_ms,
                end_ms,
              })),
            },
            {
              start_ms: 5000,
              end_ms: 9000,
              text: "Next line",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [],
            },
          ],
        },
      });
    },
    { track, playback: emptyState.playback },
  );
  const strokes = page.locator(".lyrics-page .highlighter-stroke");
  await expect(strokes.first()).toBeVisible();
  const alphas = (
    await strokes.evaluateAll((elements) =>
      elements.map((element) => (element as HTMLElement).style.maskImage),
    )
  ).map((mask) =>
    mask
      .match(/rgba\(0, 0, 0, [\d.]+\)/g)!
      .map((stop) => Number(stop.slice(14, -1))),
  );
  const quick = alphas[0][0],
    held = alphas.at(-1)!.at(-1)!,
    mark = Math.max(...alphas.flat());
  // The quick words are lighter than the held word, and the pause left a
  // darker mark than either.
  expect(quick).toBeLessThan(held);
  expect(mark).toBeGreaterThan(held);
  // The ink itself turns darker over the held word than the quick ones.
  const shades = async () =>
    (
      await strokes.evaluateAll((elements) =>
        elements.map((element) =>
          (element as HTMLElement).style.getPropertyValue("--shade"),
        ),
      )
    ).map((shade) =>
      (
        shade.match(/rgba?\(0, 0, 0, [\d.]+\)|rgb\(0 0 0 \/ [\d.]+\)/g) ?? []
      ).map((stop) => Number(stop.match(/[\d.]+(?=\)$)/)![0])),
    );
  const finished = await shades();
  expect(finished[0][0]).toBeLessThan(finished.at(-1)!.at(-1)!);
  // While a slow word is being drawn, it darkens the longer it is held.
  const holdAt = (progress_ms: number) =>
    page.evaluate(
      ({ track, playback, progress_ms }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...playback,
            item: track,
            is_playing: false,
            progress_ms,
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { track, playback: emptyState.playback, progress_ms },
    );
  await holdAt(1800);
  await page.waitForTimeout(300);
  const early = (await shades()).at(-1)!.at(-1)!;
  await holdAt(3000);
  await expect
    .poll(async () => (await shades()).at(-1)!.at(-1)!)
    .toBeGreaterThan(early);
});

test("highlighter tip glides and ink stays through the next line", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await showLyrics(page);
  const at = (progress_ms: number) =>
    page.evaluate(
      ({ track, playback, progress_ms }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...playback,
            item: track,
            is_playing: false,
            progress_ms,
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { track, playback: emptyState.playback, progress_ms },
    );
  await at(1100);
  await page.evaluate(
    ({ track }) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              // The last word runs on after the next line begins.
              start_ms: 0,
              end_ms: 8000,
              text: "Mmmmmmmm",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [{ text: "Mmmmmmmm", start_ms: 0, end_ms: 9000 }],
            },
            {
              start_ms: 8000,
              end_ms: 12000,
              text: "Next line",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [],
            },
          ],
        },
      }),
    { track },
  );
  const first = page.locator(".lyrics-page .lyric-line").first();
  const width = () =>
    first
      .locator(".highlighter-tip")
      .evaluate((tip) => tip.getBoundingClientRect().right);
  await expect(first.locator(".highlighter-stroke")).toHaveCount(1);
  const early = await width();
  // Still inside the second letter: the tip has moved on regardless.
  await at(1250);
  await expect.poll(width).toBeGreaterThan(early + 2);
  // The next line has begun while this one's last word is still running:
  // this line keeps all its ink.
  // Near the end of the word, then the line ends: the tip glides to the
  // end of the line rather than jumping there.
  await at(7000);
  await expect.poll(width).toBeGreaterThan(early + 20);
  const before = await width();
  const end = await first
    .locator(".highlighter-piece")
    .last()
    .evaluate((piece) => piece.getBoundingClientRect().right);
  await at(9500);
  const next = await page.evaluate(
    () =>
      new Promise<number>((resolve) =>
        requestAnimationFrame(() =>
          resolve(
            document
              .querySelector(".lyrics-page .lyric-line .highlighter-tip")!
              .getBoundingClientRect().right,
          ),
        ),
      ),
  );
  expect(next).toBeLessThan(end - 4);
  expect(next).toBeGreaterThanOrEqual(before - 1);
  await expect.poll(width).toBeGreaterThan(end - 14);

  await expect(page.locator(".lyrics-page .lyric-line").nth(1)).toHaveClass(
    /active/,
  );
  await expect(first.locator(".highlighter-stroke")).toHaveCount(1);
});

test("highlighter keeps a word's syllables on one row", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await showLyrics(page);
  const syllables = [
    "Elo",
    "quence ",
    "be",
    "longs ",
    "to ",
    "the ",
    "con",
    "quer",
    "or ",
    "of ",
    "ev",
    "ery ",
    "na",
    "tion",
  ];
  await page.evaluate(
    ({ track, playback, syllables }) => {
      const { emit } = (window as any).__clientTest;
      emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: false,
          progress_ms: 500,
        },
        observed_at: Date.now(),
        changed: false,
      });
      emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              start_ms: 0,
              end_ms: 7000,
              text: syllables.join(""),
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: syllables.map((text, index) => ({
                text,
                start_ms: index * 500,
                end_ms: (index + 1) * 500,
              })),
            },
          ],
        },
      });
    },
    { track, playback: emptyState.playback, syllables },
  );
  const line = page.locator(".lyrics-page .lyric-line").first();
  await expect(line.locator(".highlighter-piece")).toHaveCount(14);
  for (const width of [900, 1000, 1100, 1300, 1600]) {
    await page.setViewportSize({ width, height: 900 });
    const tops = await line
      .locator(".highlighter-piece")
      .evaluateAll((pieces) =>
        pieces.map((piece) => piece.getBoundingClientRect().top),
      );
    // "con", "quer", "or" and "na", "tion" each share a row.
    expect(new Set(tops.slice(6, 9)).size, `conqueror at ${width}px`).toBe(1);
    expect(new Set(tops.slice(12)).size, `nation at ${width}px`).toBe(1);
  }
});

test("highlighter finishes a line at the singing's pace", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  // Early scroll, on by default, ends a line 200 ms before its last word.
  await openSettings(page, "Lyrics");
  await page.getByRole("switch", { name: "Early scroll", exact: true }).check();
  await showLyrics(page);
  const at = (progress_ms: number) =>
    page.evaluate(
      ({ track, playback, progress_ms }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...playback,
            item: track,
            is_playing: false,
            progress_ms,
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { track, playback: emptyState.playback, progress_ms },
    );
  await at(3500);
  await page.evaluate(
    ({ track }) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              start_ms: 0,
              end_ms: 6000,
              text: "Slap box?",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [
                { text: "Slap ", start_ms: 0, end_ms: 2000 },
                { text: "box?", start_ms: 3000, end_ms: 6000 },
              ],
            },
            {
              start_ms: 6000,
              end_ms: 9000,
              text: "They said",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [],
            },
          ],
        },
      }),
    { track },
  );
  const first = page.locator(".lyrics-page .lyric-line").first();
  // 5.8 s: early scroll has moved on, but "box?" is still being sung.
  await at(5800);
  await expect(first).not.toHaveClass(/active/);
  // So the tip is still in "box?", not at its end.
  const spread = () =>
    first.evaluate((line) => {
      const tip = line.querySelector(".highlighter-tip:not(.lifted)")!;
      const box = line
        .querySelectorAll(".highlighter-piece")[1]
        .getBoundingClientRect();
      const middle =
        (tip.getBoundingClientRect().left + tip.getBoundingClientRect().right) /
        2;
      return (middle - box.left) / box.width;
    });
  await page.waitForTimeout(400);
  const halfway = await spread();
  expect(halfway).toBeGreaterThan(0.5);
  expect(halfway).toBeLessThan(1.02);
  // Once the word is sung the tip reaches the end.
  await at(6500);
  await expect.poll(spread).toBeGreaterThan(0.95);
});

test("highlighter ink edge sits on the letter being sung", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await showLyrics(page);
  const at = (progress_ms: number) =>
    page.evaluate(
      ({ track, playback, progress_ms }) =>
        (window as any).__clientTest.emit({
          type: "playback",
          playback: {
            ...playback,
            item: track,
            is_playing: false,
            progress_ms,
          },
          observed_at: Date.now(),
          changed: false,
        }),
      { track, playback: emptyState.playback, progress_ms },
    );
  await at(500);
  await page.evaluate(
    ({ track }) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              start_ms: 0,
              end_ms: 4000,
              text: "Mmmmmmmmmm",
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: [{ text: "Mmmmmmmmmm", start_ms: 0, end_ms: 4000 }],
            },
          ],
        },
      }),
    { track },
  );
  const first = page.locator(".lyrics-page .lyric-line").first();
  // Where the ink's edge sits, as a share of the word.
  const edge = () =>
    first.evaluate((line) => {
      const tip = line.querySelector(".highlighter-tip:not(.lifted)")!;
      const piece = line
        .querySelector(".highlighter-piece")!
        .getBoundingClientRect();
      const em = parseFloat(getComputedStyle(line).fontSize);
      const box = tip.getBoundingClientRect();
      return (
        ((box.left + box.right) / 2 - 0.09 * em - piece.left) / piece.width
      );
    });
  for (const [time, share] of [
    [1000, 0.25],
    [2000, 0.5],
    [3000, 0.75],
  ]) {
    await at(time);
    await expect.poll(edge).toBeGreaterThan(share - 0.04);
    expect(await edge()).toBeLessThan(share + 0.04);
  }
});

test("highlighter toggles mid-song without per-line layouts", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await page.evaluate(
    ({ track, playback }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: {
          ...playback,
          item: track,
          is_playing: false,
          progress_ms: 120000,
        },
        observed_at: Date.now(),
        changed: false,
      }),
    { track, playback: emptyState.playback },
  );
  await page.waitForTimeout(1000);
  await openSettings(page, "Effects");
  const session = await page.context().newCDPSession(page);
  await session.send("Performance.enable");
  const layouts = async () => {
    const { metrics } = await session.send("Performance.getMetrics");
    return metrics.find((metric) => metric.name === "LayoutCount")!.value;
  };
  for (let attempt = 0; attempt < 3; attempt++) {
    const before = await layouts();
    await page.getByRole("switch", { name: "Highlighter" }).click();
    await expect(page.locator(".highlighter-stroke")).toHaveCount(60);
    // A constant number of layouts, rather than one for each sung line.
    expect((await layouts()) - before).toBeLessThan(15);
    await page.getByRole("switch", { name: "Highlighter" }).click();
    await expect(page.locator(".highlighter-stroke")).toHaveCount(0);
  }
  await session.detach();
});

async function highlighterAt(page: Page, progress_ms: number) {
  await page.evaluate(
    ({ track, playback, progress_ms }) =>
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: false, progress_ms },
        observed_at: Date.now(),
        changed: false,
      }),
    { track, playback: emptyState.playback, progress_ms },
  );
}

async function highlighterFixture(page: Page, words: string[]) {
  await startClient(page);
  await openSettings(page, "Effects");
  await page.getByRole("switch", { name: "Highlighter" }).click();
  await showLyrics(page);
  await highlighterAt(page, 500);
  await page.evaluate(
    ({ track, words }) =>
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri: track.uri,
        lyrics: {
          synced: true,
          instrumental: false,
          source: "spicy_lyrics",
          lines: [
            {
              start_ms: 0,
              end_ms: words.length * 1000,
              text: words.join(""),
              translation: "",
              romanization: "",
              background: false,
              opposite: false,
              words: words.map((text, index) => ({
                text,
                start_ms: index * 1000,
                end_ms: (index + 1) * 1000,
              })),
            },
          ],
        },
      }),
    { track, words },
  );
  return page.locator(".lyrics-page .lyric-line").first();
}

test("highlighter grows a wrapped row's tip from zero", async ({ page }) => {
  const line = await highlighterFixture(page, [
    "First ",
    "second ",
    "third ",
    "fourth",
  ]);
  await line.locator(".highlighter-words").evaluate((root) => {
    root.style.width = "350px";
  });
  const next = await line
    .locator(".highlighter-piece")
    .evaluateAll((pieces) => {
      const firstTop = (pieces[0] as HTMLElement).offsetTop;
      const index = pieces.findIndex(
        (piece) => (piece as HTMLElement).offsetTop > firstTop,
      );
      return { index, width: pieces[index].getBoundingClientRect().width };
    });
  expect(next.index).toBeGreaterThan(0);
  // Hold the first few pixels of the second row. A full nib must not pop in.
  await highlighterAt(page, next.index * 1000 + 3000 / next.width);
  await expect(line.locator(".highlighter-tip")).toHaveCount(2);
  const tip = line.locator(".highlighter-tip").nth(1);
  await expect
    .poll(() => tip.evaluate((node) => Number(getComputedStyle(node).opacity)))
    .toBeLessThan(0.25);
  await highlighterAt(page, next.index * 1000 + 500);
  await expect(tip).toHaveCSS("opacity", "1");
  await expect(line.locator(".highlighter-tip").first()).toHaveCSS(
    "opacity",
    "0",
  );
});

test("highlighter keeps a finished row complete after resize", async ({
  page,
}) => {
  const line = await highlighterFixture(page, ["Finished"]);
  await highlighterAt(page, 2000);
  await expect(line.locator(".highlighter-stroke")).toHaveCount(1);
  const resize = async (width: string) => {
    await line.locator(".highlighter-words").evaluate((root, width) => {
      root.style.fontSize = width;
    }, width);
    await page.waitForTimeout(150);
    return line.locator(".highlighter-stroke").evaluate((stroke) => {
      const insets = (stroke as HTMLElement).style.clipPath
        .slice(6)
        .split("round")[0]
        .trim()
        .split(/\s+/);
      return parseFloat(insets[1] ?? insets[0]);
    });
  };
  await resize("30px");
  expect(await resize("80px")).toBeLessThan(1);
  await highlighterAt(page, -1);
  await expect(line.locator(".highlighter-stroke")).toHaveCount(0);
  await highlighterAt(page, 2000);
  await expect(line.locator(".highlighter-stroke")).toHaveCount(1);
  expect(await resize("100px")).toBeLessThan(1);
});

test("highlighter has no extra hop at a row's end", async ({ page }) => {
  const line = await highlighterFixture(page, ["Finish"]);
  const reaches: number[] = [];
  for (const time of [970, 980, 990, 1000]) {
    await highlighterAt(page, time);
    await page.waitForTimeout(300);
    reaches.push(
      await line.locator(".highlighter-stroke").evaluate((stroke) => {
        const node = stroke as HTMLElement;
        const insets = node.style.clipPath
          .slice(6)
          .split("round")[0]
          .trim()
          .split(/\s+/);
        return (
          parseFloat(node.style.width) - parseFloat(insets[1] ?? insets[0])
        );
      }),
    );
  }
  const step = reaches[2] - reaches[1];
  expect(reaches[3] - reaches[2]).toBeLessThan(step * 1.5 + 0.2);
});

test("highlighter cancels queued ink when seeking before a frame", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  await page.evaluate(() =>
    (window as any).__TAURI_INTERNALS__.invoke("import_lyrics"),
  );
  await highlighterAt(page, 120000);
  await openSettings(page, "Effects");
  await page.evaluate(() => {
    const request = window.requestAnimationFrame;
    const cancel = window.cancelAnimationFrame;
    const held = new Map<number, FrameRequestCallback>();
    let serial = -1;
    window.requestAnimationFrame = (callback) => {
      held.set(--serial, callback);
      return serial;
    };
    window.cancelAnimationFrame = (id) => {
      held.delete(id);
      cancel(id);
    };
    (window as any).__resumeHighlighterFrames = () => {
      window.requestAnimationFrame = request;
      window.cancelAnimationFrame = cancel;
      held.forEach((callback) => callback(performance.now()));
    };
    document
      .querySelector<HTMLElement>('[role="switch"][aria-label="Highlighter"]')!
      .click();
  });
  try {
    await expect(page.locator(".highlighter-words")).toHaveCount(80);
    await highlighterAt(page, 0);
    await expect(page.locator(".highlighter-piece").nth(59)).toHaveAttribute(
      "style",
      "--ink: 0;",
    );
  } finally {
    await page.evaluate(() => (window as any).__resumeHighlighterFrames());
  }
  // No previously queued finished line may repaint after the rewind.
  await page.waitForTimeout(150);
  await expect(page.locator(".highlighter-stroke")).toHaveCount(0);
});

test("Space Gravity over a canvas uses the left side too", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await emitCanvas(page);
  await openSettings(page, "Effects");
  await page.getByLabel("Space Gravity").check();
  await page.keyboard.press("Escape");
  await showLyrics(page);
  await page.evaluate((uri) => {
    const host = (window as any).__clientTest;
    const lines = Array.from({ length: 12 }, (_, l) => ({
      text: "",
      start_ms: l * 2000,
      end_ms: l * 2000 + 1900,
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
      words: Array.from({ length: 6 }, (_, w) => ({
        text: (w ? " " : "") + `word${l}${w}`,
        start_ms: l * 2000 + w * 300,
        end_ms: l * 2000 + w * 300 + 280,
      })),
    }));
    lines.forEach(
      (line) => (line.text = line.words.map((w) => w.text).join("")),
    );
    host.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines,
      },
    });
    host.emit({
      type: "native_position",
      uri,
      position_ms: 1000,
      playing: true,
      observed_at: Date.now(),
    });
  }, track.uri);
  await expect(page.locator(".canvas-lyrics video")).toBeVisible();
  await page
    .getByTitle(/screen/i)
    .first()
    .click();
  await expect(page.locator(".application")).toHaveClass(/immersive/);
  // Let several lines float in.
  await page.waitForTimeout(6000);
  const leftShare = await page.evaluate(() => {
    const stage = document
      .querySelector(".gravity-stage")!
      .getBoundingClientRect();
    const words = [
      ...document.querySelectorAll(".gravity-word[data-spawned]:not(.exiting)"),
    ].map((word) => {
      const box = word.getBoundingClientRect();
      return (box.left + box.right) / 2 - stage.left;
    });
    return words.filter((x) => x < stage.width / 2).length / words.length;
  });
  // Words spread over the canvas instead of crowding the right half.
  expect(leftShare).toBeGreaterThan(0.3);
});

test("the Lyrics popout button opens the popout window", async ({ page }) => {
  await startClient(page);
  await page
    .locator(".player-bar")
    .getByRole("button", { name: "Lyrics popout", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (entry: any) => entry.command === "open_lyrics_popout",
        ),
      ),
    )
    .toBe(true);
});

test("the lyrics popout shows the song, its lyrics and a seek bar", async ({
  page,
}) => {
  await startClient(page);
  await page.goto("/?view=lyrics-popout");
  const popout = page.locator(".lyrics-popout");
  await expect(popout.locator(".lyrics-popout-title strong")).toHaveText(
    track.name,
  );
  // Only the popout renders: no main window chrome.
  await expect(page.locator(".application")).toHaveCount(0);
  await page.evaluate((uri) => {
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: Array.from({ length: 30 }, (_, index) => ({
          text: `Popout line ${index + 1}`,
          start_ms: index * 2000,
          end_ms: (index + 1) * 2000,
          translation: "",
          romanization: "",
          words: [
            {
              text: `Popout line ${index + 1}`,
              start_ms: index * 2000,
              end_ms: (index + 1) * 2000,
            },
          ],
          background: false,
          opposite: false,
        })),
      },
    });
  }, track.uri);
  // The snapshot has the song 40 seconds in: line 21 is being sung.
  await expect(
    popout.locator(".lyric-line.active").filter({ hasText: "Popout line 21" }),
  ).toBeVisible();
  await expect(popout.getByLabel("Seek position")).toBeVisible();
  // Close and playback controls show only while the pointer moves.
  const controls = popout.locator(".lyrics-popout-controls");
  await expect(controls).toHaveCSS("opacity", "0");
  await page.mouse.move(400, 300);
  await page.mouse.move(420, 320);
  await expect(controls).toHaveCSS("opacity", "1");
  await expect(
    popout.getByRole("button", { name: "Close lyrics popout" }),
  ).toBeVisible();
  await popout.getByRole("button", { name: "Next song" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request.command,
      ),
    )
    .toBe("next");
  await popout
    .locator(".lyric-line")
    .filter({ hasText: "Popout line 22" })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args,
      ),
    )
    .toMatchObject({ request: { command: "seek" }, target: "native" });
});

test("lyrics open on the current line instead of sliding up to it", async ({
  page,
}) => {
  await startClient(page);
  await playLyricTrack(page, false);
  await page.evaluate(
    ({ uri }) =>
      (window as any).__clientTest.emit({
        type: "native_position",
        uri,
        position_ms: 40_500,
        playing: false,
        observed_at: Date.now(),
      }),
    { uri: track.uri },
  );
  // Record the scroll position from the first frame the lyrics show.
  await page.evaluate(() => {
    const trace: number[] = [];
    (window as any).__scrollTrace = trace;
    const sample = () => {
      const flow = document.querySelector(".lyrics-page .lyric-scroll");
      if (flow?.querySelector(".lyric-line")) trace.push(flow.scrollTop);
      if (trace.length < 40) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
  await page.getByTitle("Lyrics", { exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).__scrollTrace.length))
    .toBe(40);
  const trace: number[] = await page.evaluate(
    () => (window as any).__scrollTrace,
  );
  expect(trace.at(-1)!).toBeGreaterThan(100);
  expect(Math.abs(trace[0] - trace.at(-1)!)).toBeLessThan(8);
});

test("the discography page shows the drawing mic while releases load", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__discography = { pending: true };
  });
  await startClient(page, "discography:0123456789abcdefghijkl");
  const loading = page.getByRole("status", { name: "Loading releases" });
  await expect(loading.locator("svg.mic-loader")).toBeVisible();
  // The head runs its own loop round from the base.
  await expect
    .poll(() =>
      loading
        .locator("circle")
        .evaluate((circle) => circle.getAnimations().length),
    )
    .toBe(1);
  // Centred in the page below the header.
  const page_ = await page.locator(".discography-page").boundingBox();
  const mic = await loading.locator("svg").boundingBox();
  expect(
    Math.abs(mic!.x + mic!.width / 2 - (page_!.x + page_!.width / 2)),
  ).toBeLessThan(2);
});

test("the discography page lists each release with its songs", async ({
  page,
}) => {
  const artistId = "0123456789abcdefghijkl";
  await page.addInitScript(
    ({ track }) => {
      const release = (id: string, name: string, type: string) => ({
        id,
        uri: `spotify:album:${id}`,
        name,
        album_type: type,
        release_date: "2025",
        total_tracks: 1,
        images: [{ url: "https://artwork.example.test/cover.svg" }],
      });
      // Set before the client's own init script fills in __clientTest.
      (window as any).__discography = {
        list: [
          release("aaaaaaaaaaaaaaaaaaaaaa", "Vampire Bat", "single"),
          release("bbbbbbbbbbbbbbbbbbbbbb", "Dreamland", "album"),
        ],
        tracks: {
          aaaaaaaaaaaaaaaaaaaaaa: [
            [{ ...track, uri: "spotify:track:vampire", name: "Vampire Bat" }],
            { "spotify:track:vampire": 16051998 },
          ],
        },
      };
    },
    { track },
  );
  await startClient(page, `discography:${artistId}`);
  const discography = page.locator(".discography-page");
  const first = discography.locator(".discography-release").first();
  await expect(first.locator(".discography-release-title")).toHaveText(
    "Vampire Bat",
  );
  await expect(first).toContainText("Single • 2025 • 1 song");
  await expect(first.locator(".discography-track-title strong")).toHaveText(
    "Vampire Bat",
  );
  await expect(first).toContainText("16,051,998");
  // The filter narrows to one kind of release.
  await discography.getByRole("button", { name: "All" }).click();
  await page.getByRole("menuitemradio", { name: "Albums" }).click();
  await expect(discography.locator(".discography-release")).toHaveCount(1);
  await expect(discography.locator(".discography-release-title")).toHaveText(
    "Dreamland",
  );
  // A release's play button plays it on this computer.
  await discography.getByRole("button", { name: "Play Dreamland" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "player")
            .at(-1)?.args.request,
      ),
    )
    .toMatchObject({
      command: "play_home",
      context: "spotify:album:bbbbbbbbbbbbbbbbbbbbbb",
    });
});
test("compact lyrics line the title up with the year and artists", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 900 });
  await startClient(page);
  await playLyricTrack(page, false);
  await showLyrics(page);
  await page
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Compact lyrics", { exact: true })
    .click();
  const left = (selector: string) =>
    page
      .locator(selector)
      .first()
      .evaluate((node) => node.getBoundingClientRect().left);
  await expect(page.locator(".lyrics-page.narrow")).toHaveCount(1);
  expect(
    Math.abs(
      (await left(".lyrics-page .lyrics-title .song-title")) -
        (await left(".lyrics-page .lyrics-artists")),
    ),
  ).toBeLessThan(0.5);
});

test("Cinema Lyrics: a setting shows its button, which toggles the window", async ({
  page,
}) => {
  await startClient(page);
  const button = page
    .locator(".player-bar")
    .getByRole("button", { name: "Cinema Lyrics", exact: true });
  // Off by default, as in the fork.
  await expect(button).toHaveCount(0);
  await openSettings(page, "Layout");
  await page.getByLabel("Enable Cinema Lyrics Window", { exact: true }).check();
  await page.keyboard.press("Escape");
  await button.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (entry: any) => entry.command === "toggle_cinema_lyrics",
        ),
      ),
    )
    .toBe(true);
});

test("the Cinema window shows the full screen lyrics page and its toolbar", async ({
  page,
}) => {
  await startClient(page);
  await page.goto("/?view=cinema");
  const cinema = page.locator(".application.immersive.cinema");
  await expect(cinema.locator(".lyrics-page")).toBeVisible();
  // Only the lyrics: no top bar, library or player bar.
  await expect(
    page.locator(".topbar, .library-panel, .player-bar"),
  ).toHaveCount(0);
  await expect(cinema.locator(".lyrics-title .song-title")).toHaveText(
    track.name,
  );
  const toolbar = cinema.getByRole("toolbar", { name: "Lyrics" });
  // Its controls show while the mouse moves, as in full screen.
  await page.mouse.move(400, 300);
  await page.mouse.move(420, 320);
  await expect(
    toolbar.getByTitle("Full screen", { exact: true }),
  ).toBeVisible();
  await expect(
    toolbar.getByRole("button", { name: "Close Cinema" }),
  ).toBeVisible();
  // A toolbar toggle saves for every window.
  await toolbar.getByTitle("Compact lyrics", { exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "save_settings")
            .at(-1)?.args.settings.lyrics_compact,
      ),
    )
    .toBe(true);
  await expect(cinema.locator(".lyrics-page.narrow")).toHaveCount(1);
});

test("Cinema opens Settings over itself and Escape closes only Settings", async ({
  page,
}) => {
  await startClient(page);
  await page.goto("/?view=cinema");
  const cinema = page.locator(".application.immersive.cinema");
  await expect(cinema.locator(".lyrics-page")).toBeVisible();
  await page.mouse.move(400, 300);
  await page.mouse.move(420, 320);
  await cinema
    .getByRole("toolbar", { name: "Lyrics" })
    .getByTitle("Lyssna settings", { exact: true })
    .click();
  await expect(cinema.getByText("Lyssna settings").last()).toBeVisible();
  await expect(cinema.locator(".lyrics-page")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(cinema.getByRole("tab", { name: "Appearance" })).toHaveCount(0);
  await expect(cinema.locator(".lyrics-page")).toBeVisible();
});

test("the main window acts on the Cinema window's requests", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emitNamed("cinema-request", {
      kind: "settings",
    }),
  );
  await expect(
    page.getByRole("dialog", { name: "Lyssna settings" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.evaluate(() =>
    (window as any).__clientTest.emitNamed("cinema-request", {
      kind: "route",
      route: "album:0123456789abcdefghijkl",
    }),
  );
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__clientTest.commands
            .filter((entry: any) => entry.command === "browse")
            .at(-1)?.args.key,
      ),
    )
    .toBe("album:0123456789abcdefghijkl");
});

test("the popout and Cinema windows show lyrics in the main window's font", async ({
  page,
}) => {
  await startClient(page);
  const font = async () => {
    await page.evaluate((uri) => {
      (window as any).__clientTest.emit({
        type: "lyrics",
        uri,
        lyrics: {
          source: "Imported TTML",
          synced: true,
          instrumental: false,
          lines: [0, 1].map((index) => ({
            text: `Line ${index + 1}`,
            start_ms: index * 60000,
            end_ms: (index + 1) * 60000,
            translation: "",
            romanization: "",
            words: [],
            background: false,
            opposite: false,
          })),
        },
      });
    }, track.uri);
    const line = page.locator(".lyric-line").first();
    await expect(line).toBeVisible();
    return line.evaluate((line) => getComputedStyle(line).fontFamily);
  };
  await page.getByTitle("Lyrics", { exact: true }).click();
  const main = await font();
  expect(main).toContain("Inter Variable");
  for (const view of ["lyrics-popout", "cinema"]) {
    await page.goto(`/?view=${view}`);
    expect(await font()).toBe(main);
  }
});

test("a line keeps its size as it becomes the active line", async ({
  page,
}) => {
  await startClient(page);
  await showLyrics(page);
  const at = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms },
    );
  await page.evaluate((uri) => {
    const line = (start: number, words: string[]) => ({
      start_ms: start,
      end_ms: start + 4000,
      text: words.join(" "),
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
      words: words.map((text, index) => ({
        text: index ? ` ${text}` : text,
        start_ms: start + index * 1000,
        end_ms: start + index * 1000 + 900,
      })),
    });
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          line(0, ["I", "love,", "I", "love"]),
          line(4000, ["I've", "been", "so", "busy"]),
        ],
      },
    });
  }, track.uri);
  await at(3500);
  const second = page.locator(".lyrics-page .lyric-line").nth(1);
  // The last word, sung last, is unsung both before and after. Its drawn
  // size counts the line's own shrink too.
  const busy = () =>
    second
      .locator(".lyric-syllable")
      .last()
      .evaluate((node) => {
        const scale = (element: Element) => {
          const value = getComputedStyle(element).scale;
          return value === "none" ? 1 : Number(value);
        };
        return scale(node) * scale(node.closest(".lyric-main")!);
      });
  await expect(second).not.toHaveClass(/active/);
  const before = await busy();
  // Early scroll marks the line active a moment before it starts; its words
  // must not jump back to full size in between.
  await at(3900);
  await expect(second).toHaveClass(/active/);
  expect(Math.abs((await busy()) - before)).toBeLessThan(0.005);
  await at(4100);
  await page.waitForTimeout(300);
  expect(Math.abs((await busy()) - before)).toBeLessThan(0.005);
});

test("split words keep their size as their line finishes", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  const at = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms },
    );
  await page.evaluate((uri) => {
    const line = (start: number) => ({
      text: "spectacular",
      start_ms: start,
      end_ms: start + 3000,
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
      words: [
        ["spec", 0, 300],
        ["ta", 300, 600],
        ["cu", 600, 900],
        ["lar", 900, 1200],
      ].map(([text, from, to]) => ({
        text,
        start_ms: start + (from as number),
        end_ms: start + (to as number),
      })),
    });
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          line(0),
          {
            ...line(4000),
            text: "view",
            words: [{ text: "view", start_ms: 4000, end_ms: 4500 }],
          },
        ],
      },
    });
  }, track.uri);
  const first = page
    .locator(".lyrics-page .lyric-line")
    .filter({ hasText: "spectacular" })
    .first();
  const sizes = () =>
    first
      .locator(".lyric-syllable")
      .evaluateAll((pieces) =>
        pieces.map((piece) => piece.getBoundingClientRect().height),
      );
  // The first three pieces are sung and settled while the last is sung...
  await at(950);
  await expect(first).toHaveClass(/active/);
  await page.waitForTimeout(1500);
  const sung = (await sizes()).slice(0, 3);
  for (const height of sung)
    expect(Math.abs(height - sung[0])).toBeLessThan(0.05);
  // ...and none grows once it has.
  await at(3600);
  await expect(first).toHaveClass(/past/);
  await page.waitForTimeout(300);
  const after = (await sizes()).slice(0, 3);
  after.forEach((height, index) =>
    expect(Math.abs(height - sung[index])).toBeLessThan(0.05),
  );
});

const emitHeldLyrics = (page: Page) =>
  page.evaluate((uri) => {
    const word = (text: string, start_ms: number, end_ms: number) => ({
      text,
      start_ms,
      end_ms,
    });
    const base = {
      translation: "",
      romanization: "",
      background: false,
      opposite: false,
    };
    (window as any).__clientTest.emit({
      type: "lyrics",
      uri,
      lyrics: {
        source: "Imported TTML",
        synced: true,
        instrumental: false,
        lines: [
          {
            ...base,
            text: "Run from Dracula",
            start_ms: 1000,
            end_ms: 3600,
            words: [
              word("Run ", 1000, 1200),
              word("from ", 1200, 1300),
              word("Dra", 1300, 1600),
              word("cu", 1600, 1900),
              word("la", 1900, 3600),
            ],
          },
          {
            ...base,
            text: "next line",
            start_ms: 3700,
            end_ms: 6000,
            words: [word("next ", 3700, 4500), word("line", 4500, 6000)],
          },
        ],
      },
    });
  }, track.uri);

test("words keep their spacing from idle through sung", async ({ page }) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await emitHeldLyrics(page);
  const at = (position_ms: number) =>
    page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms },
    );
  const line = page
    .locator(".lyrics-page .lyric-line")
    .filter({ hasText: "Dracula" })
    .first();
  // The gap between "from" and "Dra", both unsung until 1200ms.
  const gap = () =>
    line.locator(".lyric-syllable").evaluateAll((pieces) => {
      const [, from, dra] = pieces.map((piece) =>
        piece.getBoundingClientRect(),
      );
      return dra.left - from.right;
    });
  await at(0);
  await page.waitForTimeout(200);
  const idle = await gap();
  // Early scroll makes the line active before it starts.
  await at(900);
  await expect(line).toHaveClass(/active/);
  await page.waitForTimeout(300);
  expect(Math.abs((await gap()) - idle)).toBeLessThan(0.75);
  await at(5000);
  await expect(line).toHaveClass(/past/);
  await page.waitForTimeout(2500);
  expect(Math.abs((await gap()) - idle)).toBeLessThan(0.75);
});

test("held letters glide back to rest once their word is sung", async ({
  page,
}) => {
  await startClient(page);
  await page.getByTitle("Lyrics", { exact: true }).click();
  await emitHeldLyrics(page);
  const line = page
    .locator(".lyrics-page .lyric-line")
    .filter({ hasText: "Dracula" })
    .first();
  await expect(line.locator(".lyric-letter")).toHaveCount(2);
  // Song time follows the clock, sampled after the held word ends.
  const start = Date.now() - 3000;
  const samples: number[][] = [];
  while (Date.now() - start < 5000) {
    await page.evaluate(
      ({ uri, position_ms }) =>
        (window as any).__clientTest.emit({
          type: "native_position",
          uri,
          position_ms,
          playing: false,
          observed_at: Date.now(),
        }),
      { uri: track.uri, position_ms: Date.now() - start },
    );
    if (Date.now() - start > 3450)
      samples.push(
        await line
          .locator(".lyric-letter")
          .evaluateAll((letters) =>
            letters.map((letter) =>
              Number(
                getComputedStyle(letter).scale === "none"
                  ? 1
                  : getComputedStyle(letter).scale,
              ),
            ),
          ),
      );
    await page.waitForTimeout(25);
  }
  // No letter dips below full size and bounces back.
  expect(samples.length).toBeGreaterThan(10);
  for (const scales of samples)
    for (const scale of scales) expect(scale).toBeGreaterThan(0.999);
});

test("About the artist shows the About section's photo, as Spotify does", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  const avatar = "https://i.scdn.co/image/avatar";
  const gallery = "https://i.scdn.co/image/gallery";
  const emit = (loaded: boolean) =>
    page.evaluate(
      ({ track, playback, avatar, gallery, loaded }) => {
        const host = (window as any).__clientTest;
        host.emit({
          type: "playback",
          playback: { ...playback, item: track, is_playing: true },
          observed_at: Date.now(),
          changed: true,
        });
        host.emit({
          type: "playing_details",
          uri: track.uri,
          details: {
            artists: [
              {
                ...track.artists[0],
                images: [{ url: avatar }],
                ...(loaded
                  ? { gallery_image: gallery, about_loaded: true }
                  : {}),
              },
            ],
            queue: [],
            queued: 0,
          },
        });
      },
      { track, playback: emptyState.playback, avatar, gallery, loaded },
    );
  // Until the About details arrive, no photo: the avatar never flashes.
  await emit(false);
  await expect(
    page.locator(".now-playing .artist-card .artist-portrait"),
  ).toBeVisible();
  await expect(
    page.locator(".now-playing .artist-card .artist-portrait img"),
  ).toHaveCount(0);
  await emit(true);
  await expect(
    page.locator(".now-playing .artist-card .artist-portrait img"),
  ).toHaveAttribute("src", gallery);
});

test("an artist's About sent ahead fills the card as the song starts", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  const gallery = "https://i.scdn.co/image/ahead";
  await page.evaluate(
    ({ track, playback, gallery }) => {
      const host = (window as any).__clientTest;
      // Warmed while the previous song played.
      host.emit({
        type: "artist_about",
        id: track.artists[0].id,
        about: { about_loaded: true, gallery_image: gallery },
      });
      // The song starts; its details have not arrived yet.
      host.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
    },
    { track, playback: emptyState.playback, gallery },
  );
  await expect(
    page.locator(".now-playing .artist-card .artist-portrait img"),
  ).toHaveAttribute("src", gallery);
});

test("the artist card's verified badge never gets cut off", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.evaluate(
    ({ track, playback }) => {
      const host = (window as any).__clientTest;
      host.emit({
        type: "playback",
        playback: { ...playback, item: track, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
      host.emit({
        type: "playing_details",
        uri: track.uri,
        details: {
          artists: [
            {
              ...track.artists[0],
              name: "John Michael Howell And A Much Longer Name",
              verified: true,
              monthly_listeners: 5_983_396,
              about_loaded: true,
            },
          ],
          queue: [],
          queued: 0,
        },
      });
    },
    { track, playback: emptyState.playback },
  );
  const name = page.locator(".now-playing .artist-card-name");
  const badge = name.locator(".verified-badge");
  await expect(badge).toBeVisible();
  const box = (await name.boundingBox())!;
  const tick = (await badge.boundingBox())!;
  expect(tick.width).toBeGreaterThan(18);
  expect(tick.x + tick.width).toBeLessThanOrEqual(box.x + box.width + 0.5);
});

test("Your Library and Now playing keep their dragged widths", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1600, height: 900 });
  await startClient(page);
  const saved = () =>
    page.evaluate(() => {
      const last = (window as any).__clientTest.commands
        .filter((entry: any) => entry.command === "save_settings")
        .at(-1);
      return last?.args.settings ?? {};
    });
  const drag = async (handle: Locator, by: number) => {
    const box = (await handle.boundingBox())!;
    const y = box.y + box.height / 2;
    await page.mouse.move(box.x + 1, y);
    await page.mouse.down();
    await page.mouse.move(box.x + 1 + by, y, { steps: 6 });
    await page.mouse.up();
  };
  await drag(page.locator(".library-resizer"), 80);
  await expect
    .poll(async () => (await saved()).library_width)
    .toBeGreaterThan(250);
  await drag(page.locator(".panel-resizer:not(.library-resizer)"), -60);
  await expect
    .poll(async () => (await saved()).now_playing_width)
    .toBeGreaterThan(360);
  // Saved widths come back with the settings on the next launch.
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    const settings = (window as any).__clientTest.commands
      .filter((entry: any) => entry.command === "save_settings")
      .at(-1).args.settings;
    host.emit({
      type: "settings",
      settings: { ...settings, library_width: 400, now_playing_width: 450 },
    });
  });
  await expect(page.locator(".library-panel")).toHaveCSS("width", "400px");
  await expect(page.locator("aside.now-playing")).toHaveCSS("width", "450px");
});

test("Listen on a creator profile plays the song in Lyssna", async ({
  page,
}) => {
  await startClient(page);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "play_uri",
      uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
    }),
  );
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "player")
          .map((entry: any) => entry.args.request),
      ),
    )
    .toContainEqual({
      command: "play",
      uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
    });
});

test("Local Files sits in Your Library with its song count", async ({
  page,
}) => {
  await startClient(page);
  const row = page.locator('[data-context-route="local"]');
  await expect(row).toContainText("Local Files");
  await page.evaluate(() =>
    (window as any).__clientTest.emit({ type: "local_files", count: 3 }),
  );
  await expect(row).toContainText("Playlist · 3 tracks");
  await row.click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands
          .filter((entry: any) => entry.command === "browse")
          .map((entry: any) => entry.args.key),
      ),
    )
    .toContain("local");
});

test("a local song has no About the artist card", async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await startClient(page);
  await page.evaluate(
    ({ track, playback }) => {
      const local = {
        ...track,
        id: undefined,
        uri: "spotify:local:Jeff+Guo:Album:Song:200",
        artists: [{ name: "Jeff Guo", uri: "" }],
        is_local: true,
        on_disk: true,
      };
      (window as any).__clientTest.emit({
        type: "playback",
        playback: { ...playback, item: local, is_playing: true },
        observed_at: Date.now(),
        changed: true,
      });
    },
    { track, playback: emptyState.playback },
  );
  await expect(page.locator(".now-playing .now-artists")).toContainText(
    "Jeff Guo",
  );
  await expect(page.locator(".now-playing .artist-card")).toHaveCount(0);
});

test("Nightly sets itself up and shows each step's progress", async ({
  page,
}) => {
  await startClient(page);
  await openSettings(page, "Advanced");
  const version = page.getByRole("combobox", { name: "Version", exact: true });
  await expect(version.locator("option")).toHaveText(["Stable", "Nightly"]);
  const calls = (command: string) =>
    page.evaluate(
      (name) =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === name,
        ).length,
      command,
    );
  await version.selectOption("nightly");
  const dialog = page.getByRole("alertdialog", { name: "Switch to Nightly?" });
  await expect(dialog).toContainText("Windows asks for permission once");
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(version).toHaveValue("stable");
  expect(await calls("nightly_build")).toBe(0);

  await version.selectOption("nightly");
  await dialog
    .getByRole("button", { name: "Build Nightly", exact: true })
    .click();
  await expect.poll(() => calls("nightly_build")).toBe(1);
  const step = (
    id: string,
    label: string,
    state: string,
    progress: number | null = null,
    detail = "",
  ) => ({ id, label, state, progress, detail });
  const report = (steps: unknown[], extra = {}) =>
    page.evaluate(
      ({ steps, extra }) =>
        (window as any).__clientTest.emit({
          type: "nightly",
          report: {
            steps,
            running: true,
            error: null,
            note: null,
            finished: false,
            log: "C:/nightly.log",
            ...extra,
          },
        }),
      { steps, extra },
    );
  await report([
    step("cpp", "C++ build tools", "skipped"),
    step("rust", "Rust", "running", 0.4, "info: downloading component 'rustc'"),
    step("app", "Building Lyssna", "pending"),
  ]);
  const building = page.getByRole("alertdialog", { name: "Building Nightly" });
  await expect(
    building.getByRole("progressbar", { name: "Rust" }),
  ).toHaveAttribute("aria-valuenow", "40");
  await expect(building).toContainText("Already there");
  await expect(building).toContainText("downloading component");
  await expect(building).not.toContainText("Still building");
  await report([
    step("cpp", "C++ build tools", "skipped"),
    step("rust", "Rust", "done", 1),
    step(
      "app",
      "Building Lyssna",
      "running",
      760 / 762,
      "Building [=======> ] 760/762: lyssna(bin)",
    ),
  ]);
  await expect(building).toContainText("Still building, don’t worry");
  await building.getByRole("button", { name: "Hide", exact: true }).click();
  await expect(building).toBeHidden();
  await expect(page.getByText(/Building Nightly · \d+%/)).toBeVisible();
  await page
    .getByRole("button", { name: "Show progress", exact: true })
    .click();
  await report(
    [
      step("cpp", "C++ build tools", "skipped"),
      step("rust", "Rust", "done", 1),
      step("app", "Building Lyssna", "failed", null, "error[E0308]"),
    ],
    { running: false, error: "cargo failed (exit code 101)" },
  );
  const stopped = page.getByRole("alertdialog", {
    name: "Nightly build stopped",
  });
  await expect(stopped.getByRole("alert")).toHaveText(
    "cargo failed (exit code 101)",
  );
  await stopped.getByRole("button", { name: "Open log", exact: true }).click();
  await expect.poll(() => calls("nightly_log")).toBe(1);
  await stopped.getByRole("button", { name: "Try again", exact: true }).click();
  await expect.poll(() => calls("nightly_build")).toBe(2);
});

test("switching to Nightly shows the build on the top bar at once", async ({
  page,
}) => {
  await startClient(page);
  const ring = page.getByRole("button", { name: /Building Nightly update/ });
  await expect(ring).toHaveCount(0);
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "nightly",
      report: {
        running: true,
        finished: false,
        error: null,
        note: null,
        log: null,
        steps: [
          {
            id: "rust",
            label: "Rust",
            state: "running",
            progress: 0.2,
            detail: null,
          },
        ],
      },
    }),
  );
  await expect(ring).toBeVisible();
  // The tag tells of the build for five seconds, then only on hover.
  const tag = page.getByRole("button", { name: /Building update · / });
  await expect(tag).toHaveCSS("opacity", "1");
  await page.mouse.move(4, 400);
  await expect(tag).toHaveCSS("opacity", "0", { timeout: 7000 });
  await ring.hover();
  await expect(tag).toHaveCSS("opacity", "1");
  await tag.click();
  const dialog = page.getByRole("alertdialog", { name: "Building Nightly" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Hide" }).click();
  await expect(dialog).toBeHidden();
  await expect(ring).toBeVisible();
  // Closing the popup hands focus back, but the tag stays hidden, and the
  // button has no native tooltip repeating it.
  await page.mouse.move(4, 400);
  await expect(tag).toHaveCSS("opacity", "0");
  await expect(
    page.getByRole("button", { name: /^Building Nightly update/ }),
  ).not.toHaveAttribute("title");
  // Focus handed back by code after a click, as WebKit does when a popup
  // closes, leaves the tag hidden; only the keyboard shows it.
  await page.mouse.click(4, 400);
  await page
    .getByRole("button", { name: /^Building Nightly update/ })
    .evaluate((button: HTMLElement) => button.focus());
  await page.waitForTimeout(400);
  await expect(tag).toHaveCSS("opacity", "0");
});

test("Nightly's build cache can be cleared from Settings", async ({ page }) => {
  await startClient(page);
  await openSettings(page, "Advanced");
  // Only on Nightly.
  await expect(page.getByText("Nightly build cache")).toHaveCount(0);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, update_channel: "nightly" },
      }),
    emptyState.settings,
  );
  const row = page.locator(".setting-row", { hasText: "Nightly build cache" });
  await row.getByRole("button", { name: "Clear cache", exact: true }).click();
  await expect(
    page.getByText("Cleared 3,072 MB of Nightly build files."),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__clientTest.commands.filter(
          (entry: any) => entry.command === "nightly_clear_cache",
        ).length,
    ),
  ).toBe(1);
});

test("the Nightly button shows a building update, then restarts into it", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__nightlyCommit = "9dac40a".padEnd(40, "0");
  });
  await startClient(page);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, update_channel: "nightly" },
      }),
    emptyState.settings,
  );
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "nightly",
      report: {
        running: true,
        finished: false,
        error: null,
        note: null,
        log: null,
        steps: [
          {
            id: "crates",
            label: "Rust packages",
            state: "done",
            progress: null,
            detail: null,
          },
          {
            id: "app",
            label: "Building Lyssna",
            state: "running",
            progress: 0.5,
            detail: null,
          },
        ],
      },
    }),
  );
  const building = page.getByRole("button", {
    name: "Building Nightly update, 55%",
  });
  await expect(building).toBeVisible();
  await expect(building.locator(".nightly-ring")).toBeVisible();
  // The prompt stays out of the way until the build is clicked.
  const prompt = page.getByRole("dialog", {
    name: "Lyssna Nightly 1234567 is available",
  });
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "update",
      status: "Building update…",
      version: "Nightly 1234567",
    }),
  );
  await expect(prompt).toBeHidden();
  await page.getByRole("button", { name: "Building update · 55%" }).click();
  await expect(prompt).toBeVisible();
  // The build can be stopped from the prompt.
  await prompt
    .getByRole("button", { name: "Cancel build", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (call: any) => call.command === "nightly_cancel",
        ),
      ),
    )
    .toBe(true);
  // Clicking outside the prompt hides it.
  await page.mouse.click(4, 4);
  await expect(prompt).toBeHidden();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.emit({
      type: "nightly",
      report: {
        running: false,
        finished: true,
        error: null,
        note: null,
        log: null,
        steps: [],
      },
    });
    host.emit({
      type: "update",
      status: "Ready to install",
      version: "Nightly 1234567",
    });
  });
  await expect(
    page.getByRole("button", { name: "Restart to update" }),
  ).toBeVisible();
  await expect(prompt).toBeHidden();
  await page.getByRole("button", { name: "Restart to update" }).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (call: any) =>
            call.command === "app_update" && call.args?.action === "install",
        ),
      ),
    )
    .toBe(true);
});

test("with automatic builds off, a new Nightly turns the button blue instead of popping up", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__nightlyCommit = "9dac40a".padEnd(40, "0");
  });
  await startClient(page);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: {
          ...settings,
          update_channel: "nightly",
          auto_download_updates: false,
        },
      }),
    emptyState.settings,
  );
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "update",
      status: "Update available",
      version: "Nightly 1234567",
    }),
  );
  const button = page.getByRole("button", {
    name: "A newer Nightly is available",
  });
  await expect(button).toBeVisible();
  // Fades from its usual colour to blue.
  await expect(button).toHaveCSS("color", "rgb(77, 155, 255)");
  const prompt = page.getByRole("dialog", {
    name: "Lyssna Nightly 1234567 is available",
  });
  await expect(prompt).toBeHidden();
  await button.click();
  await expect(prompt).toBeVisible();
  await expect(
    prompt.getByRole("button", { name: "Build update", exact: true }),
  ).toBeEnabled();
});

test("lyssna:// links open songs and pages without playing", async ({
  page,
}) => {
  await startClient(page);
  // A song link opens its album with the song selected; the album is
  // looked up natively before the interface takes the link.
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.link = {
      kind: "open",
      value: "album:0000000000000000000abc",
      select: "spotify:track:item30",
    };
    host.emit({ type: "link" });
  });
  const row = page.locator('.track-row[data-uri="spotify:track:item30"]');
  await expect(row).toHaveClass(/selected/);
  await expect(row).toBeInViewport();
  await expect(page.locator(".track-row.selected")).toHaveCount(1);
  expect(
    await page.evaluate(() =>
      (window as any).__clientTest.commands.some(
        (call: any) =>
          call.command === "player" && call.args?.request?.command === "play",
      ),
    ),
  ).toBe(false);
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.link = { kind: "open", value: "album:4uLU6hMCjMI75M1A2tKUQC" };
    host.emit({ type: "link" });
  });
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__clientTest.commands.some(
          (call: any) =>
            call.command === "browse" &&
            JSON.stringify(call.args).includes("album:4uLU6hMCjMI75M1A2tKUQC"),
        ),
      ),
    )
    .toBe(true);
});

test("a Nightly copy checks for newer Nightly code from the top bar", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__nightlyCommit = "9dac40a".padEnd(40, "0");
  });
  await startClient(page);
  const refresh = page.getByRole("button", {
    name: "Check for a newer Nightly",
  });
  // Only while following Nightly.
  await expect(refresh).toHaveCount(0);
  await page.evaluate(
    (settings) =>
      (window as any).__clientTest.emit({
        type: "settings",
        settings: { ...settings, update_channel: "nightly" },
      }),
    emptyState.settings,
  );
  await refresh.click();
  await expect(page.getByText("You're on the latest Nightly.")).toBeVisible();
  await page.evaluate(() => {
    const host = (window as any).__clientTest;
    host.nightlyCheck = "available";
    host.emit({
      type: "update",
      status: "Update available",
      version: "Nightly 1234567",
      notes:
        "1234567 new Shuffle is remembered.\nabcdef0 fixed Songs no longer pause.\nand 3 more",
    });
  });
  const prompt = page.getByRole("dialog", {
    name: "Lyssna Nightly 1234567 is available",
  });
  // It builds by itself, so it shows on the button, not over the app.
  await expect(prompt).toBeHidden();
  await refresh.click();
  await expect(prompt).toBeVisible();
  await expect(
    // Built in the background, as updates build automatically.
    prompt.getByRole("button", { name: "Building…", exact: true }),
  ).toBeDisabled();
  // Each new commit, and how far the build has got.
  await expect(prompt.getByText("You're using Nightly 9dac40a")).toBeVisible();
  await expect(
    prompt.getByRole("heading", { name: "5 commits since your Nightly" }),
  ).toBeVisible();
  await expect(prompt.getByText("9dac40a→1234567")).toBeVisible();
  await expect(
    prompt.getByRole("region", { name: "New" }).getByRole("listitem"),
  ).toHaveText(["Shuffle is remembered.1234567"]);
  await expect(
    prompt.getByRole("region", { name: "Fixed" }).getByRole("listitem"),
  ).toHaveText(["Songs no longer pause.abcdef0"]);
  await expect(prompt.getByText("and 3 older commits")).toBeVisible();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "nightly",
      report: {
        running: true,
        finished: false,
        error: null,
        note: null,
        log: null,
        steps: [
          {
            id: "crates",
            label: "Rust packages",
            state: "done",
            progress: null,
            detail: null,
          },
          {
            id: "app",
            label: "Building Lyssna",
            state: "running",
            progress: 0.5,
            detail: "Compiling librespot",
          },
        ],
      },
    }),
  );
  await expect(prompt.getByText("Building Lyssna")).toBeVisible();
  await expect(prompt.getByText("Compiling librespot")).toBeVisible();
  await expect(
    prompt.getByRole("progressbar", { name: "Building update" }),
  ).toHaveAttribute("aria-valuenow", /^[1-9]/);
  // Kept in the background while it builds; once ready the button offers the restart.
  await prompt
    .getByRole("button", { name: "Keep in background", exact: true })
    .click();
  await expect(prompt).toBeHidden();
  await page.evaluate(() =>
    (window as any).__clientTest.emit({
      type: "update",
      status: "Ready to install",
      version: "Nightly 1234567",
    }),
  );
  await expect(
    page.getByRole("button", { name: "Restart to update" }),
  ).toBeVisible();
  await expect(prompt).toBeHidden();
});

test("panel scrollers keep only the overlay bar after switching pages", async ({
  page,
}) => {
  await startClient(page);
  const scroller = page.locator(".browse-scroll");
  // Search and other pages share one scroller element with different classes.
  await page.getByPlaceholder("What do you want to play?").click();
  await page.keyboard.type("song");
  await expect(page.locator(".browse-scroll.search-scroll")).toBeVisible();
  await page
    .locator(".library-item")
    .filter({ hasText: "Liked Songs" })
    .click();
  await expect(page.locator(".track-row").first()).toBeVisible();
  await expect(scroller).not.toHaveClass(/search-scroll/);
  await expect(scroller).toHaveClass(/overlay-scrolled/);
  expect(
    await scroller.evaluate(
      (element: HTMLElement) => element.offsetWidth - element.clientWidth,
    ),
  ).toBe(0);
});
