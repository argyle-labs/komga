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

orca drives this plugin through the generic `service.*` surface:

```sh
orca service.deploy komga      # render + launch on any supported runtime
orca service.status komga      # health + rich diagnostics (typed payload)
orca service.backup komga      # location-agnostic backup (tar; PBS on Proxmox)
orca service.configure komga   # apply config via the upstream API
```

plus library upkeep tools against a registered endpoint (`komga.create`, API
key sent as `X-API-Key`, held in the secrets domain as `komga.<name>.api_key`):

```sh
orca komga.library.audit --name komga                        # read-only: numbering, duplicates, gaps
orca komga.books.renumber --name komga --all                 # dry run: planned number fixes
orca komga.books.renumber --name komga --series-id <id> --execute
orca komga.books.dedupe --name komga --all --purge-generic   # dry run: keep/delete per number
orca komga.books.dedupe --name komga --all --purge-generic --books <ids> --execute
orca komga.library.scan --name komga --library-id <id> --deep --execute
orca komga.readlist.sync --name komga --readlist 'Secret Wars' --entry 'Secret Wars (2015)#1' --entry 'Secret Wars (2015)#2'   # dry run: order, unresolved, diff
orca komga.readlist.sync --name komga --readlist 'Secret Wars' --entry ... --book-ids <ids> --execute
orca komga.series.metadata --name komga --series-id <id>
orca komga.series.metadata_set --name komga --series-id <id> --status ended --total-book-count 9 --execute
```

Komga numbers books by sort order, not by the number in the filename, so a
series with gaps reads out of order. `renumber` writes the filename's issue
number to `number`/`numberSort` with both locks, which survive rescans. It
skips locked numbers (unless `--force`), decimal (`605.1`) and suffixed
(`065.Deaths`) issues, volumes (`v07`), files with no number, and files that
look like another series.
`--execute` applies the whole plan as computed when it runs, with no confirm
list; each write is a locked metadata change another write can undo.

`dedupe` keeps one file per series and issue number: a full source over
mobile/webrip, then one with a release tag (`(Digital)` or a scanner group such
as `Shan-Empire`; Mylar tracks these names) over one without, then `(Digital)`,
then cbz/cbr over pdf, then the larger. Files are paired only when their series
parts agree (`The Amazing Spider-Man (2022) Volume 06 Issue 008` pairs with
`Amazing Spider-Man 008`), and volumes only with volumes. Never paired:
soft-deleted or unreadable books (reported by `audit` as `unavailable`),
files that look like another series, and numbers that are a guess — `#10 …
001`, facsimile editions, annuals filed in a non-annual series, or a number that
is also in the series name (`Spider-Man 2099`). `TPB`, `HC`, `Omnibus`,
`Deluxe`, `Compendium`, `Book N` and `vNN` mark collected editions.

`--purge-generic` also deletes every `<Series> (YYYY) Volume NN Issue NNN` file
and, with `--allow-orphan`, lists as `orphaned` the issues execute would leave
with no file, for Mylar to re-acquire.
`--execute` deletes, through Komga's `DELETE /api/v1/books/{id}/file`, the
`--books` ids from the dry run's `delete_book_ids` that are still in the plan
when it runs. It does not confirm the rest of the plan is unchanged, so re-run
the dry run right before executing. A purged file that is an issue's last paired
copy is marked `last_copy` (a file dedupe does not pair, such as a facsimile,
is only named in the reason as a possible copy), left out of `delete_book_ids`, and
dropped by `--execute` unless `--allow-orphan` is passed. `--books` without
`--execute` is refused. Each library that lost a file then gets
`emptyTrashAfterScan` (if off) and a scan.

Listings are read in one unpaged request and must match Komga's
`totalElements` exactly, or the tool fails rather than plan on a partial list.

Without `--library-id`/`--series-id`, tools act on the library named `comics`;
the filename rules are comic conventions. Any other library needs its id.

## Layout

- `src/` — the plugin (pure Rust): the `ServiceBackend` descriptor + the `komga.` library tools.
- `specs/komga.openapi.json` — Komga's upstream OpenAPI spec (3.1, keys sorted). `build.rs` generates the typed `komga_api` client from the operations listed in `src/operations.rs`. Refresh with `KOMGA_URL=… KOMGA_API_KEY=… scripts/refresh-spec.sh`; `cargo test` fails if a listed operation left the spec.
- `docs/` — standalone operator notes.
- [CAPABILITIES.md](CAPABILITIES.md) — the service-backend contract checklist.
- `assets/` — plugin icon.
