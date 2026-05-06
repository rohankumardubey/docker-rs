# Docker RS Desktop

Native Rust prototype for a real Docker Desktop replacement path, focused on a self-owned engine layer instead of the Docker daemon.

- Pull an image from a Docker registry
- Build an image from a local folder or a remote Git repository URL
- Run, stop, inspect, and tail logs for containers from the desktop UI

## What It Does

- Pulls public OCI images directly from registries into a native local store
- Stores manifests, configs, and layer blobs without calling `docker pull`
- Lists native local images after startup and after successful actions
- Builds metadata-only Dockerfiles without Docker by emitting a new OCI config + manifest
- Supports local folders and remote Git repository URLs as build contexts
- Shows runtime containers with state, status, ports, and per-container actions
- Bridges container execution through Docker Desktop on macOS until the native runtime lands

## Run

Start the desktop app:

```bash
cargo run
```

## Native Engine Notes

- The native pull path does not require Docker Engine.
- Public registry images work with anonymous auth today.
- The native builder currently supports `FROM`, `ENV`, `LABEL`, `EXPOSE`, `USER`, `WORKDIR`, `CMD`, and `ENTRYPOINT`.
- Container execution currently uses a Docker Desktop runtime bridge for `run`, `start`, `stop`, and `logs`.
- `RUN`, `COPY`, `ADD`, and multi-stage builds still need the next engine phase: a real rootfs executor and snapshotter.
