# Komga

Comics, manga, and books server. Web reader with OPDS support and Tachiyomi/Mihon integration.

- **Port**: 25600
- **Image**: `gotson/komga`
- **Compose**: [compose.yml](../compose.yml)

## Volumes

| Host Path | Container Path | Description |
|-----------|---------------|-------------|
| `${KOMGA_CONFIG_PATH:-/opt/appdata/komga}` | `/config` | Komga config and database |
| `${MEDIA_PATH}/comics` | `/data/media/comics` | Comics library |
| `${MEDIA_PATH}/manga` | `/data/media/manga` | Manga library |
| `${MEDIA_PATH}/books` | `/data/media/books` | Books library |

## Mobile Apps

- **Tachiyomi / Mihon** (Android) — add Komga as an OPDS/Komga source
- **Panels** (iOS) — OPDS support

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TZ` | `America/Denver` | Timezone |
| `KOMGA_IMAGE_TAG` | `latest` | Image tag |
| `KOMGA_CONFIG_PATH` | `/opt/appdata/komga` | Config directory |
| `KOMGA_PORT` | `25600` | Host port |
| `MEDIA_PATH` | `/mnt/pool/data/media` | Media library base path |

## Troubleshooting

```bash
docker logs komga
```

## Scan-on-import integration (orca)

New comics/manga land on the willow share (watcher won't fire), so scans are
pushed by the [download-client scan dispatcher](../../sabnzbd/docs/scan-dispatcher.md)
on `comics`/`manga` category completions.

**API:** Basic auth (`Authorization: Basic base64(user:pass)`),
`POST /api/v1/libraries/{libraryId}/scan`. `GET /api/v1/libraries` lists ids.
Libraries: **comics** `0PS084DCHJ1HH`, **manga** `0PS0877ZDJ0AZ`. Komga also
supports API keys (`X-API-Key`) as an alternative to Basic auth.

**Creds:** 1Password `komga (orca)` (orca vault). **Endpoint:**
`http://10.0.0.6:25600` (baldur).

**Future plugin capability:** expose `scan` + `configure` (register import hook).
See [CAPABILITIES.md](../CAPABILITIES.md).
