# Komga

Comics, manga, and books server. Web reader with OPDS support and Tachiyomi/Mihon integration.

- **Host**: <host> (<ip>)
- **Port**: 25600
- **Image**: `gotson/komga`
- **Compose**: [compose/komga/docker-compose.yml](../../compose/komga/docker-compose.yml)

## Volumes

| Host Path | Container Path | Description |
|-----------|---------------|-------------|
| `/opt/appdata/komga` | `/config` | Komga config and database |
| `/mnt/<host>/data/media/comics` | `/data/media/comics` | Comics library (NFS) |
| `/mnt/<host>/data/media/manga` | `/data/media/manga` | Manga library (NFS) |
| `/mnt/<host>/data/media/books` | `/data/media/books` | Books library (NFS) |

## Mobile Apps

- **Tachiyomi / Mihon** (Android) — add Komga as an OPDS/Komga source
- **Panels** (iOS) — OPDS support

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TZ` | `Etc/UTC` | Timezone |
| `KOMGA_IMAGE_TAG` | `latest` | Image tag |
| `KOMGA_CONFIG_PATH` | `/opt/appdata/komga` | Config directory |
| `KOMGA_PORT` | `25600` | Host port |
| `MEDIA_PATH` | `/mnt/<host>/data/media` | Media library base path |

## Note

[Kavita](kavita.md) is already deployed and serves a similar purpose (comics, manga, books). Consider consolidating to one or the other.

## Deploy

Deployed as a Portainer Git stack from `<github-org>/<repo>` main branch. Auto-updates every 5 minutes.

## Troubleshooting

```bash
docker logs komga
mount | grep <host>  # verify NFS mounts
```
