# Station branding and item pictures on the listener page

## Why

The listener page shows "Radio", a generic glyph and hashed colours whenever it has no
song to show. A station has no way to look like itself there. Shows and one-offs (a live
input, a stream, a file without tags) have no picture at all.

## Goals

1. The station sets a **name** and a **logo** in Preferences → Branding.
2. The listener page shows them when it has no song to show:
   - before Listen and after Stop;
   - on air, when the item has no cover or hides its title.
3. With no logo, the page shows the **app icon**.
4. An item (a show, a one-off, any item) can carry its **own picture**, which listeners see
   as its cover.

Non-goals: taglines, colours, favicons, branding on Icecast beyond the name, a picture for
history rows of items without a file cover (they get one only through goal 4).

Anti-goal: a logo that fails to load must not look set. The Branding page and the CLI say
why it failed; listeners then get the app icon.

## Approaches

| | How | Trade-off |
|---|---|---|
| **A. A `brand` message and `GET /brand`** (picked) | The station sends name and logo once per listener on the data channel, and serves the same as JSON. The page decides when to show it and saves the last brand per station. | Covers both cases, also on shared links. Needs the page to keep the brand for listeners it relays to. |
| B. Logo as the `cover` of `now` | The station puts the logo in when an item has none. | No page change for case 2, but the logo travels with every such item, and a page never learns the brand while each item has a cover, so case 1 fails on shared links. |
| C. Brand in the share link | Name in the link's hash. | No logo, and it changes links people already shared. |

## Design

### Change 1: station branding

- **Settings**: part `branding` (constant in `radio_model`), keys `name` (text) and `logo`
  (path to a PNG, JPEG or WebP file). Set through the existing `settings.set` command, so
  the CLI, MCP and scripts can set them. Empty name means "Radio", as today.
- **Name everywhere**: the name replaces the hard-coded `STATION_NAME`: the title of items
  that hide their own, and the Icecast stream name. Playout reads it from the engine state
  when it sends a title, so a change shows at the next item; Icecast takes it at its next
  connect.
- **Logo**: the station decodes the file and scales it to at most 512 px, as a PNG or JPEG
  small enough for one data-channel message. A file that is missing, not an image, or over
  4 MB is a failure: the Branding page shows the reason next to the field, and
  `craftradio-cli` shows it in the stream status. Listeners get no logo then.
- **To listeners**: a `brand` message `{type, name, logo: {mime, data} | null}`, kept apart
  from the per-item messages so that a station that has played nothing still sends it. It
  goes to each listener once, and again when the branding changes. `GET /brand` gives the
  same JSON to the page the station serves.
- **Page** (`site/index.html`):
  - the placeholder glyph becomes the app icon (inline SVG, 905 bytes);
  - title = item title, else brand name, else "Radio"; cover = item cover, else brand logo,
    else app icon; the same for the tab title and the media-session title and artwork;
  - on load it reads the saved brand of this station from `localStorage`, and asks
    `/brand` when the station is an address; after Stop it shows the brand again;
  - it keeps the `brand` message for listeners it relays to, as it keeps the hello;
  - an older station sends no `brand`: the page looks as today, with the app icon. An older
    page ignores `brand`.
- **Preferences**: a new page "Branding" in the Streaming group: Station name, Logo (path
  field, Browse…, Clear, a preview from theme sizes, the failure text). Restore page
  defaults clears both.

### Change 2: a picture for an item (my reading of "shows or one-offs need branding too")

- `ItemMeta` gets `picture: String` (path to an image file; empty means none). It is set
  through the existing `item.set_meta` command.
- The meta editor gets a Picture row (path, Browse…, Clear). The one-off dialog gets the
  same row, so a show gets its picture when it is scheduled.
- Listeners see the picture as the cover of that item, in every publish mode except Hide.
  It wins over the cover in the file's tags. It is scaled like the logo. History rows use
  it as their thumbnail.
- An unreadable picture shows as a failure in the meta editor; listeners then get what
  they get today (the file's own cover, else the brand logo).

Title and artist of a show are already set in the meta editor (Custom title); nothing new.

## Left to the builder

Message size limits, the exact wording of failures, the layout of the Branding page within
the theme tokens, and the tests: the brand reaches a listener of a station that has played
nothing; a changed brand is sent again; a bad logo gives a failure and no logo; an item
picture replaces the tag cover; an old-style state without `branding` behaves as today.

## Commits

One for change 1 station side, one for the page, one for Preferences, one for change 2.
