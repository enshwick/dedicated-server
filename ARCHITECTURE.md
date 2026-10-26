# Architecture

The image and the setup program install the game's dedicated server through SteamCMD and puts the
loader and the chosen mods beside it. It carries no game files: the server is
downloaded on the host.

`enshwick-server` is the image's entry point and its first process. On every
start it updates the server through SteamCMD, copies the loader and the
chosen mods from the image into the server, merges `ENSHWICK_SERVER_CONFIG`
into the server's configuration, and runs the server under Proton's Wine. A
stop signal reaches the server as `SIGINT`, on which it saves and quits.

On Windows the same program prepares the server the same way and runs it
as a service: `install` registers it, the service control manager starts
it, and a stop reaches the server as Ctrl+C through the server's console.

The image is built without a container engine: `cargo xtask image` repacks
the pinned sources into layers and appends them to the base image with
`crane`, so nothing runs inside the image while it is built.

| Path | What |
| --- | --- |
| `src/` | `enshwick-server`, the launcher and Windows service |
| `tools/xtask/` | `cargo xtask check`, the checks before a merge; `cargo xtask image`, the image; `cargo xtask dist`, the Windows archive |
| `image.toml` | the pinned base image, Proton build and loader release |
| `compose.yaml` | the image with Docker Compose |
| `charts/dedicated-server/` | the image on Kubernetes, as a Helm chart |
