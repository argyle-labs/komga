<p align="center">
  <img src="assets/icon-256.png" width="120" alt="komga" />
</p>

# komga

Komga is a media server for comics, mangas, BDs, and ebooks.

A first-party [orca](https://github.com/argyle-labs/orca) plugin (service-backend).

This repo is **self-contained** — the steps below run komga **by hand, without orca**. orca automates exactly this (same image, ports, and data) through one generic surface.

---

## Run it without orca

### Docker Compose

```yaml
# compose.yml
services:
  komga:
    image: gotson/komga:latest
    container_name: komga
    restart: unless-stopped
    ports:
      - "25600:25600/tcp"   # web UI
    volumes:
      - ./config:/config
      - /path/to/books:/books
```

```sh
docker compose up -d
```

### Other runtimes

**Podman** — the compose above works with `podman compose up -d`, or run it directly:

```sh
podman run -d --name komga --restart unless-stopped \
    -p 25600:25600/tcp \
    -v ./config:/config \
    -v /path/to/books:/books \
    gotson/komga:latest
```

**LXC** — on a container-capable LXC (e.g. a Proxmox LXC with nesting enabled) run the same image via Docker/Podman as above, or install komga from upstream directly on the guest: <https://komga.org/>.

**VM** — install komga from upstream (<https://komga.org/>) or run the same container image inside the VM; expose port `25600`.

**Unraid** — add via *Community Applications*, or *Docker → Add Container* with image `gotson/komga:latest`, port `25600`, and the volume paths above.

### Ports & data

| | |
|---|---|
| Default port | `25600` |
| Upstream | <https://komga.org/> |
| Operator notes | [komga.md](docs/komga.md) |


### Backup & restore

Back up the config/data volume(s) above — that's the whole service state (stop the container first for a clean copy). Restore by putting them back and starting it.

> With orca this is **`service.backup` / `service.restore`** — location-agnostic (docker / podman / lxc / vm), one command regardless of where komga runs. No per-service backup script.

## With orca

orca drives this plugin through the single generic `service.*` surface — no per-plugin tools:

```sh
orca service.deploy komga      # render + launch on any supported runtime
orca service.status komga      # health + rich diagnostics (typed payload)
orca service.backup komga      # location-agnostic backup (tar; PBS on Proxmox)
orca service.configure komga   # apply config via the upstream API
```

## Layout

- `src/` — the plugin (pure Rust): the `ServiceBackend` descriptor + `configure` / `status`.
- `docs/` — standalone operator notes.
- [CAPABILITIES.md](CAPABILITIES.md) — the service-backend contract checklist.
- `assets/` — plugin icon.
