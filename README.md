# Enshwick dedicated server

The Enshrouded dedicated server with the
[Enshwick](https://github.com/enshwick/enshwick) loader, as a container image
for Linux hosts: `ghcr.io/enshwick/dedicated-server`.

Enshrouded's server runs only on Windows, so the image runs it under the Wine
of a pinned [GE-Proton](https://github.com/GloriousEggroll/proton-ge-custom)
build. The image carries no game files: on every start it installs or updates
the server through SteamCMD, with an anonymous login, into its data volume.
Then it puts the loader and the chosen mods beside the server and starts it.

## Run it

```sh
docker run -d --name enshrouded -p 15637:15637/udp -v enshrouded:/data \
  -e ENSHWICK_SERVER_CONFIG='{"name": "My modded server"}' \
  --stop-timeout 90 ghcr.io/enshwick/dedicated-server:edge
```

The first start downloads several GB. The server is ready when its log says
`HostOnline`. `docker stop` makes the server save and quit.

[`compose.yaml`](compose.yaml) does the same with Docker Compose
(`docker compose up -d`), and the Helm chart in
[`charts/dedicated-server`](charts/dedicated-server) runs it on Kubernetes:

```sh
helm install enshrouded ./charts/dedicated-server --set serverConfig.name="My modded server"
```

The image is for x86-64 hosts only.

## Settings

| Variable | Default | What it does |
| --- | --- | --- |
| `ENSHWICK_MODS` | `optimizer` | the bundled mods to turn on, separated by commas or spaces; empty runs the loader alone |
| `ENSHWICK_SERVER_CONFIG` | none | a JSON object merged into `enshrouded_server.json` before every start; `null` removes a key |
| `ENSHWICK_UPDATE` | `true` | whether SteamCMD updates the server on every start |
| `ENSHWICK_STOP_TIMEOUT` | `60` | seconds the server has to save and quit before it is killed |

The server writes every key its configuration lacks into
`enshrouded_server.json` with its default, so `ENSHWICK_SERVER_CONFIG` names
only what differs, such as `{"name": "My server", "slotCount": 8}`. The user
groups and their passwords are in the same file.

Wine's own variables (`WINEDEBUG`, `WINEDLLOVERRIDES`, `WINENTSYNC`,
`WINEFSYNC`, `WINE_CPU_TOPOLOGY`) pass through to the server.

## The data volume

The container runs as user 1000 and keeps everything under `/data`:

| Path | What |
| --- | --- |
| `server/` | the game's server as SteamCMD installs it, with the loader |
| `server/savegame/` | the world; back this folder up |
| `server/logs/` | the server's logs, with the loader's in `logs/enshwick/` |
| `server/enshrouded_server.json` | the server's configuration |
| `server/mods/` | the mods; a DLL you add here yourself stays |
| `prefix/` | the Wine prefix |
| `home/` | SteamCMD and the Steam client library |

A bind mount instead of a volume must be writable by user 1000.

## Make it fast

Wine spends most of its extra time where the server waits for its threads.

- **ntsync.** On Linux 6.14 or newer, load the module (`modprobe ntsync`) and
  pass the device into the container (`--device /dev/ntsync`, or the
  `devices` lines in `compose.yaml`, or `ntsync.hostPath` in the chart).
  Without it Wine uses fsync where the kernel offers it.
- **CPU.** Prefer a CPU request without a limit; a CPU limit makes waits
  slower. The `performance` CPU governor helps on hosts that save power.
- **The optimizer mod**, on by default, tunes the server's waits under Wine
  and writes what it finds into `server/logs/enshwick/all.log`: the Wine
  version, the kernel, whether ntsync is there and the CPU limit.

## Build the image

```sh
cargo xtask image                          # target/image/dedicated-server.tar
docker load -i target/image/dedicated-server.tar
```

`cargo xtask image` needs `crane`, `gh` and `curl`, and no Docker daemon: it
appends the launcher, Proton and the loader to the base image as layers.
[`image.toml`](image.toml) pins what goes in. `--loader <zip>` takes a loader
archive built locally instead of the published one, and `--tag <reference>`
pushes to a registry.
