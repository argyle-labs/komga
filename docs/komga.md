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
