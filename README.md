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
- The native builder currently supports `FROM`, `ENV`, `LABEL`, `EXPOSE`, `USER`, `WORKDIR`, `CMD`, `ENTRYPOINT`, and `COPY` (including `--chown=<uid>:<gid>` and `--chmod=<octal>`, plus glob sources like `COPY pkg/*.json ./`).
- `COPY` produces a real gzipped tar layer in the native blob store and updates the image manifest and config (`rootfs.diff_ids`) so the built image can be exported as an OCI archive and loaded by any other OCI tool.
- Container execution currently uses a Docker Desktop runtime bridge for `run`, `start`, `stop`, and `logs`.
- `RUN`, `ADD`, and multi-stage builds still need the next engine phase: a real rootfs executor and snapshotter (`COPY` works today because it does not require executing code).
