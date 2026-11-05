# Local playback requests

Lyssna's existing loopback connection supports request/reply playback control as
well as Stream Deck commands and player/lyrics events. It never sends Spotify
credentials to clients. The app's `stream-deck.json` discovery file contains the
current port, random key and protocol version; read it from Lyssna's application
configuration directory. On Windows this is `%APPDATA%\Lyssna\config`.

Connect to `127.0.0.1` on that port. Send one JSON object per line:

```json
{"type":"hello","key":"<key from discovery file>","protocol":2}
```

The server replies with `{"type":"ready","protocol":2,"pid":1234,"windowHandle":123456}`
(using its process ID and main window handle; the handle is null outside Windows), then sends its usual
`state` and `lyrics` events. Older clients that omit `protocol` keep their existing
wire format. An older Lyssna build does not advertise protocol 2.

Requests use a positive integer ID, echoed in their response:

```json
{"type":"request","id":1,"action":"search","query":"song artist","limit":20}
{"type":"request","id":2,"action":"play","uri":"spotify:track:0123456789abcdefghijkl","positionMs":12000}
```

Responses contain `type: "response"`, `id`, `ok`, `data` and the observed player
`state`. Failures include `error`. State and lyrics events may arrive between
responses. Each connection executes requests in order. Requests are limited to
4096 bytes, search queries to 600 bytes without control characters, and search
returns at most 20 tracks.

| Action | Arguments | Behavior |
| --- | --- | --- |
| `search` | `query`, optional `limit` | Search through the signed-in account without changing the visible search page. |
| `track` | `uri` | Read track title, artists, duration and cover image. |
| `prepare` | `uri` | Load silently, pause and return to the start. |
| `play` | `uri`, optional `positionMs`, `quiet` | Play locally, then seek. `quiet: true` keeps playback silent for an external synchronized display. |
| `seek` | `uri`, `positionMs` | Seek only if that track is playing. |
| `pause` | None | Pause and release temporary audio control. |
| `release` | None | Release temporary audio control without pausing. |
| `calibrate-start` | `uri` | Save the prior playback state and play the requested track at a quiet output level. |
| `calibrate-quiet` | `uri` | Keep the calibration track quiet. |
| `calibrate-stop` | None | Restore the prior track, position and playing state. |

Playback requests target this computer rather than a selected remote device and
are rejected while a Jam owns its commands. Temporary audio control is exclusive
to one local connection. It changes output gain without changing the user's mixer
setting or saved volume. Disconnecting restores normal output and pauses playback
owned by a quiet session; calibration restores prior playback when the user has
not selected another track. Picking another song, or skipping, in Lyssna itself
ends a client's temporary output level. Clients should release unused preparation or
calibration sessions promptly. A prior playback queue is not restored.

Search and metadata replies use the same track shape as state events:
`uri`, `name`, `artists`, `image`, `duration_ms`. Position is paired with
`observed_at` in Unix milliseconds so a client can advance the clock while playing.

The ready message's optional `capabilities` list advertises `audio-analysis` when
source analysis is available. Send `{"type":"request","id":1,"action":"audio-analysis","uri":"spotify:track:0123456789abcdefghijkl"}`
on a separate authenticated connection to keep playback commands responsive.
The reply's data contains `uri`, `startMs`, `stepMs` (20), `durationMs` and `rms`.
RMS values describe up to the first 90 seconds decoded through the normal track
loader, before output gain. Playback, volume and queue are unchanged. No PCM,
audio file or credentials are returned. One source analysis runs at a time;
allow up to 60 seconds for a cold network read. Older builds omit the capability.
